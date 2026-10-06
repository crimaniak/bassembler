//! End-to-end tests for edge cases: refused starts, history shapes, resuming and the environment
//! handed to the resolver.

mod common;

use common::{Sandbox, args, conflicting_repo, repo};

#[test]
fn moved_base_blocks_a_resume_until_the_temporary_branch_is_deleted() {
    let sb = Sandbox::new();
    let a = conflicting_repo(&sb, "a");
    let project_args = args(&["--project", "a", "--issues", "PRJ-1,PRJ-2"]);
    assert_eq!(sb.run(&project_args).code, 1);
    let temp = a
        .branches()
        .into_iter()
        .find(|b| b.starts_with("temp_"))
        .expect("the aborted run leaves a temporary branch");

    // Move `v1` to a commit that the temporary branch does not contain.
    a.git(&["checkout", "-q", "-b", "side", "v1"]);
    a.commit("new-base.txt", "x\n", "new base");
    a.git(&["tag", "-f", "v1", "side"]);
    a.git(&["checkout", "-q", "main"]);

    let blocked = sb.run(&project_args);

    assert_eq!(
        blocked.code, 1,
        "stdout: {}\nstderr: {}",
        blocked.stdout, blocked.stderr
    );
    assert!(
        blocked
            .stderr
            .contains("does not start from the current base"),
        "{}",
        blocked.stderr
    );
    assert!(
        blocked.stderr.contains(&format!("git branch -D {temp}")),
        "{}",
        blocked.stderr
    );
    assert!(!a.has_branch("rel"));

    // Starting over works once the stale branch is gone.
    a.git(&["branch", "-D", &temp]);
    let resolver = sb.resolver_take_theirs("f.txt");
    let mut again = project_args.clone();
    again.extend(["--resolver", resolver.to_str().unwrap()]);
    let run = sb.run(&again);

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-1: first", "PRJ-2: third"]);
}

#[test]
fn unfinished_cherry_pick_is_refused_and_left_alone() {
    let sb = Sandbox::new();
    let a = conflicting_repo(&sb, "a");
    // Start a cherry-pick that conflicts (f.txt does not exist on v1) and leave it unfinished.
    let pick = a.git(&["rev-parse", "main"]);
    a.git(&["checkout", "-q", "-b", "work", "v1"]);
    assert!(!a.try_git(&["cherry-pick", &pick]));

    let run = sb.run(&args(&["--project", "a", "--issues", "PRJ-1,PRJ-2"]));

    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("a cherry-pick is in progress"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr.contains("git cherry-pick --abort"),
        "{}",
        run.stderr
    );
    assert!(a.dir().join(".git").join("CHERRY_PICK_HEAD").exists());
    assert_eq!(a.branches(), ["main", "work"]);
}

#[test]
fn missing_git_identity_is_reported_but_dry_run_does_not_need_one() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    a.git(&["config", "user.useConfigOnly", "true"]);
    a.git(&["config", "--unset", "user.name"]);
    a.git(&["config", "--unset", "user.email"]);
    let project_args = args(&["--project", "a", "--issues", "PRJ-1"]);

    let run = sb.run(&project_args);

    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("user.name/user.email are not set"),
        "{}",
        run.stderr
    );
    assert_eq!(a.branches(), ["main"]);

    let mut dry = project_args.clone();
    dry.push("--dry-run");
    let run = sb.run(&dry);

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
}

#[test]
fn the_same_repository_listed_twice_is_refused() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");

    // Two spellings of one directory.
    let run = sb.run(&args(&["--project", "a,./a", "--issues", "PRJ-1"]));
    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("same repository as 'a'"),
        "{}",
        run.stderr
    );

    // A worktree of the same repository counts as the same repository.
    a.git(&["worktree", "add", "-q", "../a-wt", "-b", "wt"]);
    let run = sb.run(&args(&["--project", "a,a-wt", "--issues", "PRJ-1"]));
    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("same repository as 'a'"),
        "{}",
        run.stderr
    );
    assert!(!a.has_branch("rel"));
}

#[test]
fn bad_project_directories_and_a_missing_resolver_are_listed_together() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    std::fs::create_dir(sb.path().join("plain")).unwrap();

    let run = sb.run(&args(&[
        "--project",
        "a,missing,plain",
        "--issues",
        "PRJ-1",
        "--resolver",
        "no-such-resolver.bat",
    ]));

    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("pre-flight checks failed"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("resolver:"), "{}", run.stderr);
    assert!(run.stderr.contains("missing: directory"), "{}", run.stderr);
    assert!(
        run.stderr.contains("plain: not a git repository"),
        "{}",
        run.stderr
    );
    assert_eq!(a.branches(), ["main"]);
}

#[test]
fn parallel_branches_go_oldest_tip_first_and_merge_commits_are_ignored() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.git(&["checkout", "-q", "-b", "feat"]);
    a.commit("f1.txt", "1\n", "PRJ-1: feat one");
    a.commit("f2.txt", "2\n", "PRJ-1: feat two");
    a.git(&["checkout", "-q", "main"]);
    a.commit("m1.txt", "1\n", "PRJ-1: main one");
    a.commit("m2.txt", "2\n", "PRJ-1: main two");
    a.git(&["merge", "-q", "--no-ff", "-m", "PRJ-1: merge feat", "feat"]);

    let run = sb.run(&args(&["--project", "a", "--issues", "PRJ-1"]));

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    // `feat` ends earlier than `main`, so its segment comes first, and each stays together.
    assert_eq!(
        a.subjects("rel", "v1"),
        [
            "PRJ-1: feat one",
            "PRJ-1: feat two",
            "PRJ-1: main one",
            "PRJ-1: main two"
        ]
    );
    assert!(
        run.stderr.contains("ignoring 1 merge commit(s)"),
        "{}",
        run.stderr
    );
    assert!(run.stdout.contains("4 commit(s) picked"), "{}", run.stdout);
}

#[test]
fn a_change_that_is_already_present_is_dropped_not_duplicated() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("x.txt", "x\n", "PRJ-1: add x");
    // The same change made independently on another branch (a hotfix copy).
    a.git(&["checkout", "-q", "-b", "hotfix", "v1"]);
    a.commit("x.txt", "x\n", "PRJ-1: add x (hotfix copy)");

    let run = sb.run(&args(&["--project", "a", "--issues", "PRJ-1"]));

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-1: add x"]);
    assert!(run.stdout.contains("(1 already present)"), "{}", run.stdout);
    assert!(
        run.stdout
            .contains("1 of 1 issue(s) applied, 1 commit(s) picked"),
        "{}",
        run.stdout
    );
}

#[test]
fn an_earlier_target_is_not_a_source_of_commits() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    let project_args = args(&["--project", "a", "--issues", "PRJ-1"]);
    assert_eq!(sb.run(&project_args).code, 0);

    // `rel` now holds a copy of the commit; it must not be counted as a second one.
    let mut plan = project_args.clone();
    plan.extend(["--override", "--dry-run"]);
    let dry = sb.run(&plan);
    assert_eq!(
        dry.code, 0,
        "stdout: {}\nstderr: {}",
        dry.stdout, dry.stderr
    );
    assert!(dry.stdout.contains("PRJ-1: 1 commit(s)"), "{}", dry.stdout);

    let mut again = project_args.clone();
    again.push("--override");
    let run = sb.run(&again);
    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-1: one"]);
    assert!(run.stdout.contains("1 commit(s) picked"), "{}", run.stdout);
}

#[test]
fn command_line_overrides_the_config_file_and_reads_a_json_issues_file() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    a.commit("2.txt", "x\n", "PRJ-2: two");
    std::fs::write(sb.path().join("issues.json"), r#"["PRJ-2", "PRJ-1"]"#).unwrap();
    std::fs::write(
        sb.path().join("bassembler.json"),
        r#"{ "base": "v1", "target": "cfg-target", "projects": ["nowhere"], "issues": ["PRJ-9"] }"#,
    )
    .unwrap();

    let run = sb.run(&[
        "--config",
        "bassembler.json",
        "--target",
        "rel",
        "--project",
        "a",
        "--issues",
        "issues.json",
    ]);

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert_eq!(a.branches(), ["main", "rel"]);
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-2: two", "PRJ-1: one"]);
}

#[test]
fn resolver_gets_project_issue_and_commit_in_its_environment() {
    let sb = Sandbox::new();
    let a = conflicting_repo(&sb, "a");
    let conflicting_commit = a.git(&["rev-parse", "main"]);
    let dump = sb.path().join("env.txt");
    let unix = format!(
        "git checkout --theirs -- f.txt\necho \"$BASSEMBLER_PROJECT $BASSEMBLER_ISSUE $BASSEMBLER_COMMIT\" >> \"{}\"",
        dump.display()
    );
    let windows = format!(
        "git checkout --theirs -- f.txt\necho %BASSEMBLER_PROJECT% %BASSEMBLER_ISSUE% %BASSEMBLER_COMMIT% >> \"{}\"\nexit /b 0",
        dump.display()
    );
    let resolver = sb.resolver_script("env-dump", &unix, &windows);

    let run = sb.run(&args(&[
        "--project",
        "a",
        "--issues",
        "PRJ-1,PRJ-2",
        "--resolver",
        resolver.to_str().unwrap(),
    ]));

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let seen = std::fs::read_to_string(&dump).expect("the resolver ran");
    assert_eq!(seen.trim(), format!("a PRJ-2 {conflicting_commit}"));
}
