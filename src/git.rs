use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

use crate::order::Commit;

/// `git log` format: hash, parents, committer time, author email, author time, message.
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%ae%x1f%at%x1f%B%x1e";

/// Runs git commands inside one working tree.
pub struct Git {
    dir: PathBuf,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Git { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(args)
            .current_dir(&self.dir)
            // Never wait for an editor or credentials: cherry-pick --continue must be non-interactive.
            .env("GIT_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null());
        cmd
    }

    pub fn output(&self, args: &[&str]) -> Result<Output> {
        self.command(args).output().with_context(|| {
            format!(
                "cannot run 'git {}' in '{}'",
                args.join(" "),
                self.dir.display()
            )
        })
    }

    /// Runs git and returns stdout, failing on a non-zero exit code.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.output(args)?;
        if !out.status.success() {
            bail!(
                "'git {}' failed: {}",
                args.join(" "),
                first_line(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Like `run`, feeding `input` to git's stdin.
    fn run_with_input(&self, args: &[&str], input: &str) -> Result<String> {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "cannot run 'git {}' in '{}'",
                    args.join(" "),
                    self.dir.display()
                )
            })?;
        // Write from a separate thread so a full stdout pipe can't deadlock us.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let input = input.to_string();
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let out = child.wait_with_output()?;
        writer.join().expect("stdin writer panicked")?;
        if !out.status.success() {
            bail!(
                "'git {}' failed: {}",
                args.join(" "),
                first_line(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Runs git and reports whether it exited successfully.
    pub fn check(&self, args: &[&str]) -> Result<bool> {
        Ok(self.output(args)?.status.success())
    }

    pub fn is_repo(&self) -> bool {
        self.check(&["rev-parse", "--git-dir"]).unwrap_or(false)
    }

    /// True if git knows who the committer is; without it cherry-pick can't create commits.
    pub fn has_committer_identity(&self) -> bool {
        self.check(&["var", "GIT_COMMITTER_IDENT"]).unwrap_or(false)
    }

    /// The shared git directory (the same for all worktrees of one repository), canonicalized.
    pub fn common_dir(&self) -> Result<PathBuf> {
        let out = self.run(&["rev-parse", "--git-common-dir"])?;
        let dir = self.dir.join(out.trim());
        dir.canonicalize()
            .with_context(|| format!("cannot resolve '{}'", dir.display()))
    }

    /// Resolves a branch, tag or other revision to a commit hash.
    pub fn resolve_commit(&self, rev: &str) -> Result<Option<String>> {
        let spec = format!("{rev}^{{commit}}");
        let out = self.output(&["rev-parse", "--verify", "--quiet", &spec])?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
    }

    pub fn branch_exists(&self, name: &str) -> Result<bool> {
        self.check(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
    }

    pub fn is_valid_branch_name(&self, name: &str) -> Result<bool> {
        self.check(&["check-ref-format", "--branch", name])
    }

    pub fn head(&self) -> Result<String> {
        Ok(self.run(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    /// True when tracked files have no staged or unstaged changes.
    pub fn is_clean(&self) -> Result<bool> {
        Ok(self
            .run(&["status", "--porcelain", "--untracked-files=no"])?
            .trim()
            .is_empty())
    }

    /// Name of an unfinished merge/rebase/cherry-pick/revert, if any.
    pub fn operation_in_progress(&self) -> Result<Option<&'static str>> {
        const MARKERS: [(&str, &str); 5] = [
            ("CHERRY_PICK_HEAD", "cherry-pick"),
            ("REVERT_HEAD", "revert"),
            ("MERGE_HEAD", "merge"),
            ("rebase-merge", "rebase"),
            ("rebase-apply", "rebase/am"),
        ];
        for (marker, op) in MARKERS {
            if self.git_path(marker)?.exists() {
                return Ok(Some(op));
            }
        }
        Ok(None)
    }

    pub fn cherry_pick_in_progress(&self) -> Result<bool> {
        Ok(self.git_path("CHERRY_PICK_HEAD")?.exists())
    }

    fn git_path(&self, name: &str) -> Result<PathBuf> {
        let p = self.run(&["rev-parse", "--git-path", name])?;
        Ok(self.dir.join(p.trim()))
    }

    pub fn unmerged_files(&self) -> Result<Vec<String>> {
        Ok(self
            .run(&["diff", "--name-only", "--diff-filter=U"])?
            .lines()
            .map(String::from)
            .collect())
    }

    /// True when the index has nothing to commit relative to HEAD.
    pub fn index_matches_head(&self) -> Result<bool> {
        self.check(&["diff", "--cached", "--quiet"])
    }

    /// Commits reachable from any ref for which `exclude` returns false, and not reachable
    /// from `base`. HEAD is not considered: it may point at an excluded branch.
    pub fn commits_since(&self, base: &str, exclude: impl Fn(&str) -> bool) -> Result<Vec<Commit>> {
        let refs = self.run(&["for-each-ref", "--format=%(refname)"])?;
        let mut revs: String = refs
            .lines()
            .filter(|r| !r.is_empty() && !exclude(r))
            .map(|r| format!("{r}\n"))
            .collect();
        revs.push_str(&format!("^{base}\n"));
        let out = self.run_with_input(&["log", "--stdin", "--no-color", LOG_FORMAT], &revs)?;
        self.parse_log(&out)
    }

    /// Commits on local branch `branch` that are not reachable from `base`.
    pub fn commits_on(&self, branch: &str, base: &str) -> Result<Vec<Commit>> {
        let revs = format!("refs/heads/{branch}\n^{base}\n");
        let out = self.run_with_input(&["log", "--stdin", "--no-color", LOG_FORMAT], &revs)?;
        self.parse_log(&out)
    }

    /// True if `ancestor` is reachable from `descendant` (or the same commit).
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool> {
        self.check(&["merge-base", "--is-ancestor", ancestor, descendant])
    }

    fn parse_log(&self, out: &str) -> Result<Vec<Commit>> {
        let mut commits = Vec::new();
        for record in out.split('\x1e') {
            let record = record.trim_start_matches(['\r', '\n']);
            if record.is_empty() {
                continue;
            }
            let mut fields = record.splitn(6, '\x1f');
            let (Some(hash), Some(parents), Some(time), Some(email), Some(atime), Some(body)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                bail!("unexpected 'git log' output in '{}'", self.dir.display());
            };
            commits.push(Commit {
                hash: hash.to_string(),
                parents: parents.split_whitespace().map(String::from).collect(),
                time: time.trim().parse().unwrap_or(0),
                subject: body.lines().next().unwrap_or("").trim().to_string(),
                author: format!("{email}\u{1f}{}", atime.trim()),
            });
        }
        Ok(commits)
    }
}

pub fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error")
        .to_string()
}
