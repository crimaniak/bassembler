# bassembler

[![CI](https://github.com/crimaniak/bassembler/actions/workflows/ci.yml/badge.svg)](https://github.com/crimaniak/bassembler/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Assembles a Git branch in one or more repositories from the commits of chosen issues, as described by `bassembler.json` (see [docs/DESIGN.md](docs/DESIGN.md) for the original specification).

Typical use: you have a release base (a tag or branch) and a list of issue codes such as `PRJ-101`. bassembler finds every commit whose subject mentions one of those issues, in each repository, cherry-picks them onto a new branch in a sensible order, optionally calls a resolver tool on conflicts, and prints a summary.

## Requirements

- `git` 2.23 or newer on `PATH`.
- Optional, for the ready-made resolvers: the [`claude` CLI](https://docs.claude.com/en/docs/claude-code) on `PATH`. Note that they send the conflicting files and commit to Claude, so use them only with code you may share with it.
- Rust 1.85 or newer to build from source (edition 2024).

Platforms: developed and used on Windows; the code is not Windows-only and CI also builds on Linux and macOS, but the Windows path is the best tested.

## Install

```
cargo install bassembler          # once published on crates.io
cargo install --git https://github.com/crimaniak/bassembler
```

Or download a binary from the GitHub Releases page, or build from a checkout:

```
cargo build --release             # target/release/bassembler (bassembler.exe on Windows)
```

## Example

```
$ bassembler --base v1.4.0 --target release-1.4-hotfix --project ../backend,../frontend --issues PRJ-101,PRJ-117 --dry-run
$ bassembler --base v1.4.0 --target release-1.4-hotfix --project ../backend,../frontend --issues PRJ-101,PRJ-117 \
    --resolver=resolvers/resolve_with_claude.ps1 --conflict=skip

Branch 'release-1.4-hotfix' from 'v1.4.0':
  ../backend           head 1d6d1b9d63
  ../frontend          head dd71890d9e
Issues:
  PRJ-101  applied    ../backend 2, ../frontend 1
  PRJ-117  SKIPPED    ../backend: 49c0208d7c "PRJ-117: ...": conflict in src/app.rs
Summary: 1 of 2 issue(s) applied, 3 commit(s) picked; 1 skipped, 0 without commits
```

## Config (`bassembler.json`)

```json
{
  "base": "v1.4.0",
  "target": "release-1.4-hotfix",
  "projects": ["../backend", "../frontend"],
  "issues": ["PRJ-101", "PRJ-117"],
  "resolver": "resolvers/resolve_with_claude.ps1",
  "conflict": "skip"
}
```

| Field      | Meaning |
|------------|---------|
| `base`     | Branch or tag the new branch starts from. Must exist in every project. |
| `target`   | Branch to create. |
| `projects` | Project directories (git working trees), relative to the config file. |
| `issues`   | Either an inline array of issue codes, or the name of a file (relative to the config file). |
| `resolver` | Optional. Same as `--resolver`. A path with a directory part is relative to the config file; a bare name is looked up on `PATH`, unless a file of that name sits next to the config file. |
| `conflict` | Optional, `"abort"` (default) or `"skip"`. Same as `--conflict`. |

The issues file format is detected automatically: a JSON array of strings, or plain text with one issue per line (blank lines and `#` comments are ignored).

Every field is optional in the file: a command-line parameter overrides the file's value. `base`, `target`, `projects` and `issues` must be given by one or the other; `resolver` and `conflict` are optional everywhere (`--override` is command-line only). With all four on the command line the config file isn't needed.

## Usage

```
bassembler [--config <path>] [--base <ref>] [--target <branch>] [--project <dir>[,<dir>...]] [--issues <codes|file>] [--override] [--resolver=<utility>] [--conflict=abort|skip] [--dry-run]
```

- `--config`: config file (default `bassembler.json` in the current directory, used if it exists; a file given explicitly must exist).
- `--base`, `--target`: override `base` and `target`.
- `--project <dir>` (alias `--projects`): override `projects`. Comma-separated or repeated. Relative paths are relative to the **current directory** (in the file they are relative to the config file).
- `--issues <codes>` (alias `--issue`): override `issues`. Comma-separated or repeated codes, e.g. `--issues ABC-1,ABC-2`. A single value that names an existing file is read as an issues file (JSON array or one code per line), relative to the current directory.
- `--override`: if `target` exists, rename it to `<target>_backup_<YYYYMMDD-HHMMSS>` (UTC) instead of failing. This happens only when the new branch is finished, so a failed run never costs you the old one.
- `--resolver=<utility>` (or `resolver` in the file): on a cherry-pick conflict, run this executable or `.bat`/`.cmd` in the project directory. It must exit with `0` if it resolved the conflict or `1` if it didn't. It gets the environment variables `BASSEMBLER_PROJECT`, `BASSEMBLER_ISSUE` and `BASSEMBLER_COMMIT`. After a `0` exit, tracked files are staged (`git add -u`) and the cherry-pick continues. A relative path is resolved from the current directory. A bare name is looked up on `PATH`; batch files need their extension. On Windows a `.ps1` script is run through PowerShell (`powershell.exe -NoProfile -ExecutionPolicy Bypass -File`, or `pwsh.exe` if that is missing), so `resolvers/resolve_with_claude.ps1` can be used directly; on other platforms a `.ps1` is just an executable file. The resolver is checked before anything changes (see Checks below).
- `--conflict=abort` (default; or `"conflict"` in the file): stop at the first unresolved conflict. The work stays on the temporary branches, so running again continues from there (see Continuing an interrupted run).
- `--conflict=skip`: drop the issue from **all** projects (branches already processed for it are reset) and go on.
- `--dry-run`: run the checks and list the commits that would be picked, without changing anything. Uncommitted changes are only a warning here.

Exit codes: `0` success, `2` finished but some issues were skipped, `1` error or aborted.

### Ready-made resolvers

`resolvers/resolve_with_claude.ps1` (Windows) and `resolvers/resolve_with_claude.sh` (Linux/macOS) ask the `claude` CLI to resolve the conflict, then check that nothing is left unmerged and no conflict markers remain. The `.sh` needs `chmod +x resolvers/resolve_with_claude.sh` and `claude` on `PATH`.

## Continuing an interrupted run

All work happens on a temporary branch named `temp_<base>_<target>_<hash>` in each project, and only a finished run renames it to `target`. If a run stops early (a conflict with `--conflict=abort`, an error, Ctrl+C, a crash), the temporary branches stay. Run `bassembler` again with the same `base`, `target` and issues (the resolver and `--conflict` don't matter) and it continues:

- Commits already on the temporary branch are recognised by author, author date and message, which a cherry-pick keeps, so commits whose conflicts were resolved are recognised too. They are not picked again. `--dry-run` shows them as `(already picked)`.
- Each project continues on its own: one that got further than the others just has fewer commits left.
- An issue skipped with `--conflict=skip` leaves no commits on the branch, so it is tried again on the next run.
- The temporary branch must still start from the current `base`. If `base` has moved on, the run stops and tells you to delete the old temporary branch (`git branch -D <name>`) to start over.
- If git was stopped in the middle of a cherry-pick, finish it (`git cherry-pick --continue`) or drop it (`git cherry-pick --abort`) first.
- Temporary branches are never used as a source of commits.

## How it works

1. **Checks** (before anything changes; all problems are listed together):
   - environment: `git` runs and is 2.23 or newer; the resolver, if any, exists, is a file, is executable (Unix) or on `PATH` (bare name), and for a Windows `.ps1` PowerShell is available; `base` and `target` differ; no two projects are the same repository (or worktrees of one);
   - each project: the directory is a git repo, `target` is a valid branch name, git `user.name`/`user.email` are set (not needed with `--dry-run`, since nothing is committed), no merge/rebase/cherry-pick is in progress, the working tree is clean, `base` resolves, and `target` doesn't exist (unless `--override` is given).
2. **Collecting commits**: takes every commit reachable from any branch, tag or remote ref but not from `base`. It skips `HEAD`, `refs/stash`, the target branch and its `_backup_` copies, locally and on remotes, since they already hold picked copies. A commit belongs to an issue when the first line of its message contains the issue code as a whole token: `ABC-1` matches `[ABC-1] fix` but not `ABC-12`. If a commit mentions several issues, the first issue in the list wins. Merge commits are ignored.
3. **Ordering**: commits come in history order, parents before children. A linear branch segment is kept together, and among parallel segments the one whose last commit is oldest (by committer time) goes first.
4. **Temporary branch**: the work is done on `temp_<base>_<target>_<hash>` (the hash covers the issue list, in order), created at `base` and checked out. If that branch already exists from an earlier run, it is checked out and the run continues on it.
5. **Applying**: issues are applied in list order, each with `git cherry-pick` in every project that has commits for it. Commits already on the temporary branch are skipped. A commit whose change is already on the branch (the pick comes out empty) is dropped and counted as "already present". When all issues are done, an existing `target` is backed up (with `--override`) and the temporary branch is renamed to `target`.
6. **Report**: a compact per-project and per-issue summary.

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

See [CONTRIBUTING.md](CONTRIBUTING.md). Changes are listed in [CHANGELOG.md](CHANGELOG.md).

## License

[MIT](LICENSE)
