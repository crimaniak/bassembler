# Contributing

Issues and pull requests are welcome.

## Before sending a pull request

```
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks on Linux, Windows and macOS. Please add a test for new behavior and a line to
`CHANGELOG.md` under *Unreleased*.

Do not include real project data (issue keys, commit messages, repository names) in tests, examples or
bug reports.

## Design

[docs/DESIGN.md](docs/DESIGN.md) holds the original specification; the README describes the current behavior.

By contributing you agree that your contribution is licensed under the MIT license.
