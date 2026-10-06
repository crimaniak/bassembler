//! Helpers for end-to-end tests: throwaway git repositories and a way to run the real binary.
#![allow(dead_code)]

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Keeps git from reading the developer's own configuration, so tests behave the same everywhere.
fn isolate(cmd: &mut Command, root: &Path) {
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("no-such-gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0");
}

/// A temporary directory holding the repositories of one test; removed on drop.
pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("bassembler-e2e-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Sandbox { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Creates an empty repository `name` on branch `main` with an identity and fixed settings.
    pub fn repo(&self, name: &str) -> Repo {
        let repo = Repo {
            dir: self.root.join(name),
            root: self.root.clone(),
            clock: Cell::new(0),
        };
        fs::create_dir_all(&repo.dir).unwrap();
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test User"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    /// Runs `bassembler` with the sandbox as the working directory.
    pub fn run(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_bassembler"));
        cmd.current_dir(&self.root).args(args);
        isolate(&mut cmd, &self.root);
        let out = cmd.output().expect("cannot run bassembler");
        Run {
            code: out.status.code().expect("bassembler was killed"),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Writes a resolver that takes the picked commit's version of `file`.
    pub fn resolver_take_theirs(&self, file: &str) -> PathBuf {
        let unix = format!("git checkout --theirs -- {file}");
        let windows = format!("git checkout --theirs -- {file}\nexit /b %ERRORLEVEL%");
        self.resolver_script("take-theirs", &unix, &windows)
    }

    /// Writes a resolver that gives up.
    pub fn resolver_fail(&self) -> PathBuf {
        self.resolver_script("fail", "exit 1", "exit /b 1")
    }

    pub fn resolver_script(&self, name: &str, unix: &str, windows: &str) -> PathBuf {
        if cfg!(windows) {
            let path = self.root.join(format!("{name}.bat"));
            let body = windows.replace('\n', "\r\n");
            fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
            path
        } else {
            let path = self.root.join(format!("{name}.sh"));
            fs::write(&path, format!("#!/bin/sh\n{unix}\n")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Git marks object files read-only, which stops remove_dir_all on Windows.
        fn writable(dir: &Path) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = fs::metadata(&path) {
                    let mut perm = meta.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = fs::set_permissions(&path, perm);
                    if meta.is_dir() {
                        writable(&path);
                    }
                }
            }
        }
        writable(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub struct Repo {
    dir: PathBuf,
    root: PathBuf,
    clock: Cell<u64>,
}

impl Repo {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Runs git in the repository and returns its trimmed stdout; panics if git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.dir).args(args);
        isolate(&mut cmd, &self.root);
        let out = cmd.output().expect("git must be installed");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            self.dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Writes `content` to `file` and commits it with `subject`. Every commit gets a later,
    /// distinct timestamp, so history order is deterministic.
    pub fn commit(&self, file: &str, content: &str, subject: &str) {
        let n = self.clock.get() + 1;
        self.clock.set(n);
        let date = format!("{} +0000", 1_600_000_000 + n * 60);
        fs::write(self.dir.join(file), content).unwrap();
        self.git(&["add", file]);
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.dir)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .args(["commit", "-q", "-m", subject]);
        isolate(&mut cmd, &self.root);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Runs git and reports whether it succeeded, for commands that are expected to fail.
    pub fn try_git(&self, args: &[&str]) -> bool {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.dir).args(args);
        isolate(&mut cmd, &self.root);
        cmd.output()
            .expect("git must be installed")
            .status
            .success()
    }

    pub fn tag(&self, name: &str) {
        self.git(&["tag", name]);
    }

    /// Subjects of the commits on `branch` that are not on `base`, oldest first.
    pub fn subjects(&self, branch: &str, base: &str) -> Vec<String> {
        let range = format!("{base}..{branch}");
        self.git(&["log", "--reverse", "--format=%s", &range])
            .lines()
            .map(String::from)
            .collect()
    }

    /// Contents of `path` as of `rev`.
    pub fn show(&self, rev: &str, path: &str) -> String {
        self.git(&["show", &format!("{rev}:{path}")])
    }

    pub fn branches(&self) -> Vec<String> {
        self.git(&["branch", "--format=%(refname:short)"])
            .lines()
            .map(String::from)
            .collect()
    }

    pub fn has_branch(&self, name: &str) -> bool {
        self.branches().iter().any(|b| b == name)
    }

    /// Every ref with the commit it points to; equal strings mean nothing was changed.
    pub fn refs(&self) -> String {
        self.git(&["for-each-ref"])
    }

    pub fn is_clean(&self) -> bool {
        self.git(&["status", "--porcelain"]).is_empty()
    }
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A repository with one base commit tagged `v1`.
pub fn repo(sb: &Sandbox, name: &str) -> Repo {
    let repo = sb.repo(name);
    repo.commit("base.txt", "base\n", "base");
    repo.tag("v1");
    repo
}

/// A repository where picking `PRJ-2` after `PRJ-1` conflicts in `f.txt`, because the unrelated
/// middle commit changed the same line.
pub fn conflicting_repo(sb: &Sandbox, name: &str) -> Repo {
    let repo = repo(sb, name);
    repo.commit("f.txt", "one\n", "PRJ-1: first");
    repo.commit("f.txt", "two\n", "unrelated change");
    repo.commit("f.txt", "three\n", "PRJ-2: third");
    repo
}

/// Command-line parameters for base `v1` and target `rel`, followed by `extra`.
pub fn args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec!["--base", "v1", "--target", "rel"];
    v.extend_from_slice(extra);
    v
}
