use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use time::OffsetDateTime;
use time::macros::format_description;

use crate::config::Config;
use crate::git::{Git, first_line};
use crate::order::{self, Commit};
use crate::{preflight, resolver, temp};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictMode {
    /// Stop processing; issues applied so far stay on the target branches.
    Abort,
    /// Skip the issue in all projects and continue with the next one.
    Skip,
}

pub struct Options {
    pub override_target: bool,
    pub resolver: Option<PathBuf>,
    pub conflict: ConflictMode,
    pub dry_run: bool,
}

struct Project {
    name: String,
    git: Git,
    base: String,
    target_exists: bool,
    backup: Option<String>,
    /// The temporary branch the work is done on until it is finished.
    temp: String,
    /// The temporary branch was left by an earlier, interrupted run.
    temp_exists: bool,
    /// Commits to pick, per issue (same indexing as `Config::issues`).
    commits: Vec<Vec<Commit>>,
    /// Same shape as `commits`: true for a commit already on the temporary branch.
    applied: Vec<Vec<bool>>,
}

impl Project {
    fn applied_count(&self) -> usize {
        self.applied.iter().flatten().filter(|&&a| a).count()
    }
}

#[derive(Default, Clone, Copy)]
struct Stats {
    picked: usize,
    resolved: usize,
    empty: usize,
    /// Already on the temporary branch from an earlier run.
    resumed: usize,
}

enum Outcome {
    NoCommits,
    Applied(Vec<(usize, Stats)>),
    Skipped(Failure),
    Aborted(Failure),
    NotProcessed,
}

struct Failure {
    project: usize,
    commit: Commit,
    reason: String,
}

/// Time spent in the conflict resolver.
#[derive(Default)]
struct ResolverTime {
    calls: Cell<usize>,
    total: Cell<Duration>,
}

enum Pick {
    Applied,
    Resolved,
    Empty,
    Failed(String),
}

/// Runs the whole assembly and prints the report. Returns the process exit code.
pub fn run(cfg: &Config, opts: &Options) -> Result<i32> {
    let started = Instant::now();
    let mut projects = check_projects(cfg, opts)?;
    collect_commits(cfg, &mut projects)?;

    if opts.dry_run {
        print_plan(cfg, &projects);
        return Ok(0);
    }

    prepare_branches(&projects)?;
    let resolver_time = ResolverTime::default();
    let outcomes = apply_issues(cfg, opts, &projects, &resolver_time)?;
    // After an abort the temporary branches stay, so that running again continues from them.
    let finished = !outcomes.iter().any(|o| matches!(o, Outcome::Aborted(_)));
    if finished {
        finalize(cfg, &mut projects)?;
    }
    Ok(print_report(
        cfg,
        &projects,
        &outcomes,
        &resolver_time,
        started.elapsed(),
        finished,
    ))
}

/// Step 1: validate every project before anything is changed.
fn check_projects(cfg: &Config, opts: &Options) -> Result<Vec<Project>> {
    let mut errors = Vec::new();
    let mut projects = Vec::new();

    // Without a working git every per-project check would fail with the same noise.
    if let Err(e) = preflight::check_git() {
        bail!("pre-flight checks failed:\n  {e}");
    }
    if cfg.base == cfg.target {
        errors.push(format!("base and target are both '{}'", cfg.base));
    }
    if let Some(resolver) = &opts.resolver {
        if let Err(e) = resolver::check(resolver) {
            errors.push(format!("resolver: {e}"));
        }
    }
    // Repositories seen so far (shared git directory, project name): two entries for the same
    // repository, or for worktrees of one, would fight over the target branch.
    let mut seen: Vec<(PathBuf, String)> = Vec::new();

    for p in &cfg.projects {
        let git = Git::new(&p.dir);
        let mut fail = |msg: String| errors.push(format!("{}: {msg}", p.name));

        if !p.dir.is_dir() {
            fail(format!("directory '{}' does not exist", p.dir.display()));
            continue;
        }
        if !git.is_repo() {
            fail("not a git repository".into());
            continue;
        }
        if let Ok(common) = git.common_dir() {
            if let Some((_, other)) = seen.iter().find(|(dir, _)| *dir == common) {
                fail(format!("same repository as '{other}'"));
                continue;
            }
            seen.push((common, p.name.clone()));
        }
        // Cherry-picking creates commits; a dry run creates none.
        if !opts.dry_run && !git.has_committer_identity() {
            fail("git user.name/user.email are not set (git config user.name / user.email)".into());
        }
        if !git.is_valid_branch_name(&cfg.target)? {
            fail(format!("'{}' is not a valid branch name", cfg.target));
            continue;
        }
        if let Some(op) = git.operation_in_progress()? {
            let hint = if op == "cherry-pick" {
                " (finish it with 'git cherry-pick --continue' or drop it with 'git cherry-pick --abort', then run again)"
            } else {
                ""
            };
            fail(format!("a {op} is in progress{hint}"));
        }
        if !git.is_clean()? {
            if opts.dry_run {
                eprintln!("{}: warning: working tree has uncommitted changes", p.name);
            } else {
                fail("working tree has uncommitted changes".into());
            }
        }
        let base = git.resolve_commit(&cfg.base)?;
        if base.is_none() {
            fail(format!("base '{}' not found", cfg.base));
        }
        let temp = temp::name(cfg);
        if !git.is_valid_branch_name(&temp)? {
            fail(format!("'{temp}' is not a valid branch name"));
            continue;
        }
        let temp_exists = git.branch_exists(&temp)?;
        if let (Some(base), true) = (&base, temp_exists) {
            if !git.is_ancestor(base, &format!("refs/heads/{temp}"))? {
                fail(format!(
                    "branch '{temp}' from an earlier run does not start from the current base '{}'; delete it (git branch -D {temp}) to start over",
                    cfg.base
                ));
            }
        }
        let target_exists = git.branch_exists(&cfg.target)?;
        if target_exists && !opts.override_target {
            fail(format!(
                "target branch '{}' already exists (use --override to back it up)",
                cfg.target
            ));
        }

        if let Some(base) = base {
            projects.push(Project {
                name: p.name.clone(),
                git,
                base,
                target_exists,
                backup: None,
                temp,
                temp_exists,
                commits: Vec::new(),
                applied: Vec::new(),
            });
        }
    }

    if !errors.is_empty() {
        bail!("pre-flight checks failed:\n  {}", errors.join("\n  "));
    }
    Ok(projects)
}

/// Step 3: find commits mentioning each issue, in history order.
fn collect_commits(cfg: &Config, projects: &mut [Project]) -> Result<()> {
    // Previously assembled branches contain copies of the same commits; don't pick those.
    let is_target = |branch: &str| {
        branch == cfg.target
            || branch
                .strip_prefix(cfg.target.as_str())
                .is_some_and(|rest| rest.starts_with("_backup_"))
            || temp::is_temp_branch(branch)
    };
    let exclude = |refname: &str| {
        if refname == "refs/stash" {
            return true;
        }
        if let Some(branch) = refname.strip_prefix("refs/heads/") {
            return is_target(branch);
        }
        // refs/remotes/<remote>/<branch>
        refname
            .strip_prefix("refs/remotes/")
            .and_then(|r| r.split_once('/'))
            .is_some_and(|(_, branch)| is_target(branch))
    };

    for p in projects {
        let all = order::order(p.git.commits_since(&p.base, exclude)?);
        p.commits = vec![Vec::new(); cfg.issues.len()];
        let mut merges = 0;
        for c in all {
            let Some(i) = cfg
                .issues
                .iter()
                .position(|issue| mentions(&c.subject, issue))
            else {
                continue;
            };
            if c.is_merge() {
                merges += 1;
                continue;
            }
            p.commits[i].push(c);
        }
        if merges > 0 {
            eprintln!(
                "{}: ignoring {merges} merge commit(s) that mention issues",
                p.name
            );
        }
        mark_applied(p)?;
    }
    Ok(())
}

/// True if `subject` contains `issue` as a whole token, so "ABC-1" doesn't match "ABC-12".
fn mentions(subject: &str, issue: &str) -> bool {
    let word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let check_before = issue.chars().next().is_some_and(word);
    let check_after = issue.chars().next_back().is_some_and(word);
    subject.match_indices(issue).any(|(pos, _)| {
        let before = subject[..pos].chars().next_back();
        let after = subject[pos + issue.len()..].chars().next();
        !(check_before && before.is_some_and(word)) && !(check_after && after.is_some_and(word))
    })
}

/// Finds which of the commits to pick are already on the temporary branch of an earlier run.
/// A cherry-pick keeps author, author date and message, so those identify a copy.
fn mark_applied(p: &mut Project) -> Result<()> {
    let present: HashSet<String> = if p.temp_exists {
        p.git
            .commits_on(&p.temp, &p.base)?
            .iter()
            .map(Commit::fingerprint)
            .collect()
    } else {
        HashSet::new()
    };
    p.applied = p
        .commits
        .iter()
        .map(|per_issue| {
            per_issue
                .iter()
                .map(|c| present.contains(&c.fingerprint()))
                .collect()
        })
        .collect();
    Ok(())
}

/// Step 2: check out each project's temporary branch, creating it at the base commit unless an
/// interrupted run left it there.
fn prepare_branches(projects: &[Project]) -> Result<()> {
    for p in projects {
        if p.temp_exists {
            p.git
                .run(&["checkout", "-q", &p.temp])
                .with_context(|| format!("{}: cannot check out branch '{}'", p.name, p.temp))?;
            let total: usize = p.commits.iter().map(Vec::len).sum();
            eprintln!(
                "{}: continuing on '{}' ({} of {total} commit(s) already picked)",
                p.name,
                p.temp,
                p.applied_count()
            );
        } else {
            p.git
                .run(&["checkout", "-q", "--no-track", "-b", &p.temp, &p.base])
                .with_context(|| format!("{}: cannot create branch '{}'", p.name, p.temp))?;
        }
    }
    Ok(())
}

/// Step 5: give the finished temporary branches their final name, backing up an existing
/// target branch first. Only now, so a failed run never costs the old branch.
fn finalize(cfg: &Config, projects: &mut [Project]) -> Result<()> {
    let stamp = timestamp();
    for p in projects {
        if p.git.branch_exists(&cfg.target)? {
            let mut backup = format!("{}_backup_{stamp}", cfg.target);
            let mut n = 2;
            while p.git.branch_exists(&backup)? {
                backup = format!("{}_backup_{stamp}_{n}", cfg.target);
                n += 1;
            }
            p.git
                .run(&["branch", "-m", &cfg.target, &backup])
                .with_context(|| format!("{}: cannot back up branch '{}'", p.name, cfg.target))?;
            p.backup = Some(backup);
        }
        p.git
            .run(&["branch", "-m", &p.temp, &cfg.target])
            .with_context(|| {
                format!("{}: cannot rename '{}' to '{}'", p.name, p.temp, cfg.target)
            })?;
    }
    Ok(())
}

/// Step 4: cherry-pick issue by issue; an issue is applied to all projects or to none.
fn apply_issues(
    cfg: &Config,
    opts: &Options,
    projects: &[Project],
    resolver_time: &ResolverTime,
) -> Result<Vec<Outcome>> {
    let mut outcomes: Vec<Outcome> = Vec::with_capacity(cfg.issues.len());

    for (i, issue) in cfg.issues.iter().enumerate() {
        let involved: Vec<usize> = (0..projects.len())
            .filter(|&p| !projects[p].commits[i].is_empty())
            .collect();
        if involved.is_empty() {
            outcomes.push(Outcome::NoCommits);
            continue;
        }
        eprintln!("applying {issue}...");

        let mut done: Vec<(usize, String, Stats)> = Vec::new();
        let mut failure = None;
        'projects: for &pi in &involved {
            let p = &projects[pi];
            let mut stats = Stats::default();
            done.push((pi, p.git.head()?, stats));
            for (j, c) in p.commits[i].iter().enumerate() {
                if p.applied[i][j] {
                    stats.resumed += 1;
                    done.last_mut().unwrap().2 = stats;
                    continue;
                }
                match pick(p, c, issue, opts.resolver.as_ref(), resolver_time)? {
                    Pick::Applied => stats.picked += 1,
                    Pick::Resolved => {
                        stats.picked += 1;
                        stats.resolved += 1;
                    }
                    Pick::Empty => stats.empty += 1,
                    Pick::Failed(reason) => {
                        failure = Some(Failure {
                            project: pi,
                            commit: c.clone(),
                            reason,
                        });
                        break 'projects;
                    }
                }
                done.last_mut().unwrap().2 = stats;
            }
        }

        let Some(failure) = failure else {
            outcomes.push(Outcome::Applied(
                done.into_iter().map(|(p, _, s)| (p, s)).collect(),
            ));
            continue;
        };

        // Move every touched branch back to where it was before this issue.
        for (pi, head, _) in &done {
            projects[*pi]
                .git
                .run(&["reset", "-q", "--hard", head])
                .with_context(|| {
                    format!("{}: cannot roll back issue {issue}", projects[*pi].name)
                })?;
        }
        match opts.conflict {
            ConflictMode::Skip => outcomes.push(Outcome::Skipped(failure)),
            ConflictMode::Abort => {
                outcomes.push(Outcome::Aborted(failure));
                break;
            }
        }
    }

    while outcomes.len() < cfg.issues.len() {
        outcomes.push(Outcome::NotProcessed);
    }
    Ok(outcomes)
}

fn pick(
    p: &Project,
    c: &Commit,
    issue: &str,
    resolver: Option<&PathBuf>,
    resolver_time: &ResolverTime,
) -> Result<Pick> {
    let git = &p.git;
    let out = git.output(&["cherry-pick", &c.hash])?;
    if out.status.success() {
        return Ok(Pick::Applied);
    }
    if !git.cherry_pick_in_progress()? {
        // Git refused to start, e.g. untracked files would be overwritten.
        return Ok(Pick::Failed(first_line(&out.stderr)));
    }

    let conflicts = git.unmerged_files()?;
    if conflicts.is_empty() && git.index_matches_head()? {
        // The change is already on the branch.
        git.run(&["cherry-pick", "--skip"])?;
        return Ok(Pick::Empty);
    }
    let reason = if conflicts.is_empty() {
        first_line(&out.stderr)
    } else {
        format!("conflict in {}", conflicts.join(", "))
    };

    if let Some(resolver) = resolver.filter(|_| !conflicts.is_empty()) {
        eprintln!(
            "{}: conflict on {} {}, running resolver",
            p.name,
            c.short(),
            c.subject
        );
        let start = Instant::now();
        let resolved = match run_resolver(resolver, p, c, issue) {
            Ok(resolved) => resolved,
            Err(e) => {
                // Leave the repository clean, not in the middle of a cherry-pick.
                let _ = git.output(&["cherry-pick", "--abort"]);
                return Err(e);
            }
        };
        let spent = start.elapsed();
        resolver_time.calls.set(resolver_time.calls.get() + 1);
        resolver_time.total.set(resolver_time.total.get() + spent);
        eprintln!(
            "{}: resolver {} after {}",
            p.name,
            if resolved { "succeeded" } else { "failed" },
            format_duration(spent)
        );
        if resolved {
            git.run(&["add", "-u"])?;
            if git.unmerged_files()?.is_empty() {
                if git.index_matches_head()? {
                    git.run(&["cherry-pick", "--skip"])?;
                    return Ok(Pick::Empty);
                }
                if git.check(&["cherry-pick", "--continue"])? {
                    return Ok(Pick::Resolved);
                }
            }
        }
    }

    let _ = git.output(&["cherry-pick", "--abort"]);
    Ok(Pick::Failed(reason))
}

/// Runs the user's resolver in the project directory. Exit code 0 means resolved.
fn run_resolver(resolver: &Path, p: &Project, c: &Commit, issue: &str) -> Result<bool> {
    let mut command = resolver::command(resolver);
    let status = command
        .current_dir(p.git.dir())
        .env("BASSEMBLER_PROJECT", &p.name)
        .env("BASSEMBLER_ISSUE", issue)
        .env("BASSEMBLER_COMMIT", &c.hash)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("cannot run resolver '{}'", resolver.display()))?;
    Ok(status.success())
}

fn print_plan(cfg: &Config, projects: &[Project]) {
    println!(
        "Dry run: '{}' from '{}' in {} project(s)",
        cfg.target,
        cfg.base,
        projects.len()
    );
    for p in projects {
        let backup = if p.target_exists {
            ", target exists and would be backed up"
        } else {
            ""
        };
        let resume = if p.temp_exists {
            let total: usize = p.commits.iter().map(Vec::len).sum();
            format!(
                ", would continue on '{}' ({} of {total} commit(s) already picked)",
                p.temp,
                p.applied_count()
            )
        } else {
            String::new()
        };
        println!(
            "  {}: base {}{resume}{backup}",
            p.name,
            &p.base[..10.min(p.base.len())]
        );
    }
    for (i, issue) in cfg.issues.iter().enumerate() {
        let total: usize = projects.iter().map(|p| p.commits[i].len()).sum();
        if total == 0 {
            println!("{issue}: no commits");
            continue;
        }
        println!("{issue}: {total} commit(s)");
        for p in projects.iter().filter(|p| !p.commits[i].is_empty()) {
            println!("  {}:", p.name);
            for (j, c) in p.commits[i].iter().enumerate() {
                let done = if p.applied[i][j] {
                    "  (already picked)"
                } else {
                    ""
                };
                println!("    {} {}{done}", c.short(), c.subject);
            }
        }
    }
}

/// Step 5: compact summary. Returns the exit code: 0 success, 2 some issues skipped, 1 aborted.
fn print_report(
    cfg: &Config,
    projects: &[Project],
    outcomes: &[Outcome],
    resolver_time: &ResolverTime,
    elapsed: Duration,
    finished: bool,
) -> i32 {
    println!();
    if finished {
        println!("Branch '{}' from '{}':", cfg.target, cfg.base);
    } else {
        println!(
            "Stopped: the work is on temporary branch(es) '{}'; '{}' was not created. Run again with the same parameters to continue.",
            projects.first().map(|p| p.temp.as_str()).unwrap_or(""),
            cfg.target
        );
    }
    for p in projects {
        let head = p.git.head().unwrap_or_default();
        let backup = p
            .backup
            .as_ref()
            .map(|b| format!(", old branch saved as '{b}'"))
            .unwrap_or_default();
        println!(
            "  {:<20} head {}{backup}",
            p.name,
            &head[..10.min(head.len())]
        );
    }

    let width = cfg.issues.iter().map(|i| i.len()).max().unwrap_or(0);
    let (mut applied, mut skipped, mut missing, mut commits, mut resumed) = (0, 0, 0, 0, 0);
    let mut aborted = false;
    println!("Issues:");
    for (issue, outcome) in cfg.issues.iter().zip(outcomes) {
        let line = match outcome {
            Outcome::NoCommits => {
                missing += 1;
                "no commits found".to_string()
            }
            Outcome::Applied(per) => {
                applied += 1;
                let parts: Vec<String> = per
                    .iter()
                    .map(|(pi, s)| {
                        commits += s.picked;
                        resumed += s.resumed;
                        let mut part = format!("{} {}", projects[*pi].name, s.picked);
                        if s.resolved > 0 {
                            part += &format!(" ({} resolved)", s.resolved);
                        }
                        if s.empty > 0 {
                            part += &format!(" ({} already present)", s.empty);
                        }
                        if s.resumed > 0 {
                            part += &format!(" (+{} from the earlier run)", s.resumed);
                        }
                        part
                    })
                    .collect();
                format!("applied    {}", parts.join(", "))
            }
            Outcome::Skipped(f) => {
                skipped += 1;
                format!("SKIPPED    {}", describe(projects, f))
            }
            Outcome::Aborted(f) => {
                aborted = true;
                format!("ABORTED    {}", describe(projects, f))
            }
            Outcome::NotProcessed => "not processed".to_string(),
        };
        println!("  {issue:<width$}  {line}");
    }

    println!(
        "Summary: {applied} of {} issue(s) applied, {commits} commit(s) picked{}; {skipped} skipped, {missing} without commits{}",
        cfg.issues.len(),
        if resumed > 0 {
            format!(" (+{resumed} from the earlier run)")
        } else {
            String::new()
        },
        if aborted { "; ABORTED" } else { "" }
    );
    let resolver = match resolver_time.calls.get() {
        0 => String::new(),
        n => format!(
            ", conflict resolver {} in {n} call(s) ({:.0}%)",
            format_duration(resolver_time.total.get()),
            100.0 * resolver_time.total.get().as_secs_f64()
                / elapsed.as_secs_f64().max(f64::EPSILON)
        ),
    };
    println!("Time: total {}{resolver}", format_duration(elapsed));
    if aborted {
        1
    } else if skipped > 0 {
        2
    } else {
        0
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    match (secs / 3600, secs % 3600 / 60, secs % 60) {
        (0, 0, s) => format!("{:.1}s", d.as_secs_f64().max(s as f64)),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, s) => format!("{h}h {m:02}m {s:02}s"),
    }
}

fn describe(projects: &[Project], f: &Failure) -> String {
    format!(
        "{}: {} \"{}\": {}",
        projects[f.project].name,
        f.commit.short(),
        f.commit.subject,
        f.reason
    )
}

/// UTC time as `YYYYMMDD-HHMMSS`, used for backup branch names.
fn timestamp() -> String {
    format_timestamp(OffsetDateTime::now_utc())
}

fn format_timestamp(t: OffsetDateTime) -> String {
    t.format(format_description!(
        "[year][month][day]-[hour][minute][second]"
    ))
    .expect("a fixed numeric format cannot fail")
}

#[cfg(test)]
mod tests {
    use super::{format_timestamp, mentions};
    use time::macros::datetime;

    #[test]
    fn issue_matching() {
        assert!(mentions("ABC-1: fix crash", "ABC-1"));
        assert!(mentions("[ABC-1] fix", "ABC-1"));
        assert!(mentions("fix (ABC-1)", "ABC-1"));
        assert!(!mentions("ABC-12: other", "ABC-1"));
        assert!(!mentions("XABC-1 other", "ABC-1"));
        assert!(mentions("ABC-12, ABC-1 both", "ABC-1"));
        assert!(mentions("fix#12", "#12"));
    }

    #[test]
    fn timestamp_format() {
        assert_eq!(
            format_timestamp(datetime!(2026-10-06 03:09:05 UTC)),
            "20261006-030905"
        );
        assert_eq!(
            format_timestamp(datetime!(2024-02-29 23:59:59 UTC)),
            "20240229-235959"
        );
        assert_eq!(
            format_timestamp(datetime!(1970-01-01 0:00 UTC)),
            "19700101-000000"
        );
    }
}
