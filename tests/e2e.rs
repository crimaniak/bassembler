//! End-to-end tests: build small git repositories, run the real binary, inspect the branches.

mod common;

use common::{Sandbox, args, conflicting_repo, repo};

#[test]
fn assembles_issues_in_list_order_across_projects() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("a2.txt", "2\n", "PRJ-2: add a2");
    a.commit("other.txt", "x\n", "unrelated work");
    a.commit("a1.txt", "1\n", "PRJ-1: add a1");
    let b = repo(&sb, "b");
    b.commit("b1.txt", "1\n", "PRJ-1: add b1");

    let run = sb.run(&args(&["--project", "a,b", "--issues", "PRJ-1,PRJ-2"]));

    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    // The issue list decides the order, not the history order.
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-1: add a1", "PRJ-2: add a2"]);
    assert_eq!(b.subjects("rel", "v1"), ["PRJ-1: add b1"]);
    assert_eq!(a.show("rel", "a1.txt"), "1");
    assert!(
        run.stdout.contains("Branch 'rel' from 'v1':"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("Summary: 2 of 2 issue(s) applied, 3 commit(s) picked; 0 skipped"),
        "{}",
        run.stdout
    );
    // The temporary branch was renamed to the target, nothing else is left.
    assert_eq!(a.branches(), ["main", "rel"]);
    assert_eq!(b.branches(), ["main", "rel"]);
    assert!(a.is_clean() && b.is_clean());
}

#[test]
fn issue_code_must_match_as_a_whole_token() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-12: a different issue");
    a.commit("2.txt", "x\n", "PRJ-1: plain");
    a.commit("3.txt", "x\n", "[PRJ-1] bracketed");

    let run = sb.run(&args(&["--project", "a", "--issues", "PRJ-1"]));

    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(
        a.subjects("rel", "v1"),
        ["PRJ-1: plain", "[PRJ-1] bracketed"]
    );
}

#[test]
fn issue_without_commits_is_reported_not_failed() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: real");

    let run = sb.run(&args(&["--project", "a", "--issues", "PRJ-1,PRJ-404"]));

    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(run.stdout.contains("no commits found"), "{}", run.stdout);
    assert!(run.stdout.contains("1 without commits"), "{}", run.stdout);
}

#[test]
fn settings_can_come_from_a_config_file_with_an_issues_file() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    a.commit("2.txt", "x\n", "PRJ-2: two");
    std::fs::write(
        sb.path().join("issues.txt"),
        "# release 1\nPRJ-2\n\nPRJ-1\n",
    )
    .unwrap();
    std::fs::write(
        sb.path().join("bassembler.json"),
        r#"{ "base": "v1", "target": "rel", "projects": ["a"], "issues": "issues.txt" }"#,
    )
    .unwrap();

    let run = sb.run(&["--config", "bassembler.json"]);

    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-2: two", "PRJ-1: one"]);
}

#[test]
fn dry_run_changes_nothing() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    let before = a.refs();

    let run = sb.run(&args(&["--project", "a", "--issues", "PRJ-1", "--dry-run"]));

    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert!(run.stdout.contains("PRJ-1: 1 commit(s)"), "{}", run.stdout);
    assert!(run.stdout.contains("PRJ-1: one"), "{}", run.stdout);
    assert_eq!(a.refs(), before);
    assert!(a.is_clean());
}

#[test]
fn resolver_fixes_a_conflict() {
    let sb = Sandbox::new();
    let a = conflicting_repo(&sb, "a");
    let resolver = sb.resolver_take_theirs("f.txt");

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
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-1: first", "PRJ-2: third"]);
    assert_eq!(a.show("rel", "f.txt"), "three");
    assert!(run.stderr.contains("resolver succeeded"), "{}", run.stderr);
    assert!(run.stdout.contains("(1 resolved)"), "{}", run.stdout);
    assert!(run.stdout.contains("conflict resolver"), "{}", run.stdout);
    assert!(a.is_clean());
}

#[test]
fn skipped_issue_is_dropped_from_every_project() {
    let sb = Sandbox::new();
    // `ok` takes PRJ-2 cleanly and is processed first; `bad` then conflicts on it.
    let ok = repo(&sb, "ok");
    ok.commit("g1.txt", "x\n", "PRJ-1: ok one");
    ok.commit("g2.txt", "x\n", "PRJ-2: ok two");
    let bad = conflicting_repo(&sb, "bad");
    let resolver = sb.resolver_fail();

    let run = sb.run(&args(&[
        "--project",
        "ok,bad",
        "--issues",
        "PRJ-1,PRJ-2",
        "--resolver",
        resolver.to_str().unwrap(),
        "--conflict=skip",
    ]));

    assert_eq!(
        run.code, 2,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    assert!(run.stderr.contains("resolver failed"), "{}", run.stderr);
    assert!(run.stdout.contains("SKIPPED"), "{}", run.stdout);
    assert!(run.stdout.contains("conflict in f.txt"), "{}", run.stdout);
    assert!(
        run.stdout.contains("1 of 2 issue(s) applied"),
        "{}",
        run.stdout
    );
    // PRJ-2 is gone from `ok` too, although it applied there.
    assert_eq!(ok.subjects("rel", "v1"), ["PRJ-1: ok one"]);
    assert_eq!(bad.subjects("rel", "v1"), ["PRJ-1: first"]);
    // The failed pick was aborted: no half-finished cherry-pick, clean trees.
    assert!(ok.is_clean() && bad.is_clean());
    assert!(!bad.dir().join(".git").join("CHERRY_PICK_HEAD").exists());
}

#[test]
fn abort_keeps_the_temporary_branch_and_a_rerun_continues() {
    let sb = Sandbox::new();
    let a = conflicting_repo(&sb, "a");
    let project_args = args(&["--project", "a", "--issues", "PRJ-1,PRJ-2"]);

    // No resolver: the conflict stops the run.
    let stopped = sb.run(&project_args);

    assert_eq!(
        stopped.code, 1,
        "stdout: {}\nstderr: {}",
        stopped.stdout, stopped.stderr
    );
    assert!(stopped.stdout.contains("ABORTED"), "{}", stopped.stdout);
    assert!(!a.has_branch("rel"));
    let temp: Vec<String> = a
        .branches()
        .into_iter()
        .filter(|b| b.starts_with("temp_"))
        .collect();
    assert_eq!(temp.len(), 1, "branches: {:?}", a.branches());
    assert_eq!(a.subjects(&temp[0], "v1"), ["PRJ-1: first"]);
    assert!(a.is_clean());

    // Same parameters plus a resolver: continues from the temporary branch.
    let resolver = sb.resolver_take_theirs("f.txt");
    let mut again = project_args.clone();
    again.extend(["--resolver", resolver.to_str().unwrap()]);
    let finished = sb.run(&again);

    assert_eq!(
        finished.code, 0,
        "stdout: {}\nstderr: {}",
        finished.stdout, finished.stderr
    );
    assert!(
        finished.stdout.contains("from the earlier run"),
        "{}",
        finished.stdout
    );
    assert_eq!(a.subjects("rel", "v1"), ["PRJ-1: first", "PRJ-2: third"]);
    assert_eq!(a.branches(), ["main", "rel"]);
}

#[test]
fn existing_target_needs_override_and_is_backed_up() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    let project_args = args(&["--project", "a", "--issues", "PRJ-1"]);
    assert_eq!(sb.run(&project_args).code, 0);
    let first_head = a.git(&["rev-parse", "rel"]);

    let refused = sb.run(&project_args);

    assert_eq!(refused.code, 1);
    assert_eq!(a.git(&["rev-parse", "rel"]), first_head);
    assert_eq!(a.branches(), ["main", "rel"]);

    let mut with_override = project_args.clone();
    with_override.push("--override");
    let run = sb.run(&with_override);

    assert_eq!(
        run.code, 0,
        "stdout: {}\nstderr: {}",
        run.stdout, run.stderr
    );
    let backups: Vec<String> = a
        .branches()
        .into_iter()
        .filter(|b| b.starts_with("rel_backup_"))
        .collect();
    assert_eq!(backups.len(), 1, "branches: {:?}", a.branches());
    assert_eq!(a.git(&["rev-parse", &backups[0]]), first_head);
    assert!(a.has_branch("rel"));
}

#[test]
fn preflight_problems_are_listed_together_and_nothing_is_created() {
    let sb = Sandbox::new();
    let a = repo(&sb, "a");
    a.commit("1.txt", "x\n", "PRJ-1: one");
    // Uncommitted change in a tracked file.
    std::fs::write(a.dir().join("base.txt"), "modified\n").unwrap();
    // No `v1` here, so the base does not exist.
    let b = sb.repo("b");
    b.commit("x.txt", "x\n", "PRJ-1: other");

    let run = sb.run(&args(&["--project", "a,b", "--issues", "PRJ-1"]));

    assert_eq!(run.code, 1);
    assert!(
        run.stderr.contains("pre-flight checks failed"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("a:"), "{}", run.stderr);
    assert!(run.stderr.contains("b:"), "{}", run.stderr);
    assert_eq!(a.branches(), ["main"]);
    assert_eq!(b.branches(), ["main"]);
}
