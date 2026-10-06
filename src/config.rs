use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::assemble::ConflictMode;

/// Contents of `bassembler.json`, with paths resolved relative to the config file.
#[derive(Debug)]
pub struct Config {
    /// Branch or tag the target branch starts from.
    pub base: String,
    /// Branch to create.
    pub target: String,
    /// Project directories (git working trees) to operate on.
    pub projects: Vec<Project>,
    /// Issue codes, in the order they are applied.
    pub issues: Vec<String>,
    /// Conflict-resolver program, if one is configured. A path with a directory part is relative
    /// to the current directory by now; a bare name is looked up on `PATH`.
    pub resolver: Option<PathBuf>,
    /// What to do with an unresolved conflict.
    pub conflict: ConflictMode,
}

#[derive(Debug)]
pub struct Project {
    /// Name as written in the config, used in messages.
    pub name: String,
    pub dir: PathBuf,
}

/// Every field may be absent: a command-line value overrides the file's, and a missing one is
/// only an error if neither supplies it.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(alias = "base_commit")]
    base: Option<String>,
    #[serde(alias = "target_branch")]
    target: Option<String>,
    projects: Option<Vec<String>>,
    /// Either an inline list of issues or the name of a file containing them.
    #[serde(alias = "issues_file")]
    issues: Option<IssuesSpec>,
    resolver: Option<String>,
    conflict: Option<ConflictMode>,
}

/// Values given on the command line. Each one that is `Some` replaces the config file's value.
#[derive(Debug, Default)]
pub struct Overrides {
    pub base: Option<String>,
    pub target: Option<String>,
    /// Directories relative to the current directory (not to the config file).
    pub projects: Option<Vec<String>>,
    /// Issue codes, or the name of a file with them (relative to the current directory).
    pub issues: Option<IssuesSpec>,
    /// Relative to the current directory.
    pub resolver: Option<PathBuf>,
    pub conflict: Option<ConflictMode>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum IssuesSpec {
    List(Vec<String>),
    File(String),
}

/// Loads the config file (if any) and applies the command-line overrides.
///
/// `path` is `None` when `--config` was not given: `bassembler.json` is then used if it exists,
/// and its absence is fine as long as the overrides supply everything. An explicitly given file
/// must exist.
pub fn load(path: Option<&Path>, over: Overrides) -> Result<Config> {
    let default = Path::new("bassembler.json");
    let (path, explicit) = match path {
        Some(p) => (p, true),
        None => (default, false),
    };
    let (raw, root) = if explicit || path.exists() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config file '{}'", path.display()))?;
        let raw: RawConfig = serde_json::from_str(strip_bom(&text))
            .with_context(|| format!("invalid config file '{}'", path.display()))?;
        let root = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        (raw, root)
    } else {
        (RawConfig::default(), PathBuf::from("."))
    };

    let base = over.base.or(raw.base).map(|b| b.trim().to_string());
    let base = match base {
        Some(b) if !b.is_empty() => b,
        Some(_) => bail!("'base' must not be empty"),
        None => bail!("'base' is not set (config file or --base)"),
    };
    let target = over.target.or(raw.target).map(|t| t.trim().to_string());
    let target = match target {
        Some(t) if !t.is_empty() => t,
        Some(_) => bail!("'target' must not be empty"),
        None => bail!("'target' is not set (config file or --target)"),
    };

    // Paths from the command line are relative to the current directory, those from the file to
    // the file itself.
    let (project_names, projects_root) = match (over.projects, raw.projects) {
        (Some(p), _) => (p, PathBuf::from(".")),
        (None, Some(p)) => (p, root.clone()),
        (None, None) => bail!("'projects' is not set (config file or --project)"),
    };
    if project_names.is_empty() {
        bail!("'projects' must list at least one directory");
    }

    let root_for_resolver = root.clone();
    let (issues_spec, issues_root) = match (over.issues, raw.issues) {
        (Some(i), _) => (i, PathBuf::from(".")),
        (None, Some(i)) => (i, root),
        (None, None) => bail!("'issues' is not set (config file or --issues)"),
    };
    let issues = match issues_spec {
        IssuesSpec::List(list) => normalize(list),
        IssuesSpec::File(file) => {
            let file = issues_root.join(file);
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read issues file '{}'", file.display()))?;
            parse_issues(&text)
                .with_context(|| format!("invalid issues file '{}'", file.display()))?
        }
    };
    if issues.is_empty() {
        bail!("the issues list is empty");
    }

    let projects = project_names
        .into_iter()
        .map(|name| Project {
            dir: projects_root.join(&name),
            name,
        })
        .collect();

    // A resolver from the file is relative to the file, like the project paths. A bare name stays
    // one (looked up on PATH) unless a file of that name sits next to the config.
    let resolver = match (over.resolver, raw.resolver) {
        (Some(r), _) => Some(r),
        (None, Some(r)) if r.trim().is_empty() => bail!("'resolver' must not be empty"),
        (None, Some(r)) => {
            let beside = root_for_resolver.join(&r);
            let r = PathBuf::from(r);
            Some(if r.components().count() > 1 || beside.exists() {
                beside
            } else {
                r
            })
        }
        (None, None) => None,
    };
    let conflict = over
        .conflict
        .or(raw.conflict)
        .unwrap_or(ConflictMode::Abort);

    Ok(Config {
        base,
        target,
        projects,
        issues,
        resolver,
        conflict,
    })
}

/// Parses an issues file: a JSON array of strings, or plain text with one issue per line.
/// Blank lines and lines starting with `#` are ignored in the text format.
pub fn parse_issues(text: &str) -> Result<Vec<String>> {
    let text = strip_bom(text);
    if text.trim_start().starts_with('[') {
        let list: Vec<String> =
            serde_json::from_str(text).context("expected a JSON array of strings")?;
        return Ok(normalize(list));
    }
    Ok(normalize(
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect(),
    ))
}

/// Trims entries, drops empty ones and duplicates, keeping the first occurrence.
fn normalize(list: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for issue in list {
        let issue = issue.trim();
        if !issue.is_empty() && !out.iter().any(|i| i == issue) {
            out.push(issue.to_string());
        }
    }
    out
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_json() {
        let v = parse_issues(" [\"A-1\", \"B-2\", \"A-1\", \" \"]").unwrap();
        assert_eq!(v, ["A-1", "B-2"]);
    }

    #[test]
    fn issues_text() {
        let v = parse_issues("\u{feff}A-1\r\n\r\n# comment\n  B-2  \n").unwrap();
        assert_eq!(v, ["A-1", "B-2"]);
    }

    fn write_config(name: &str, json: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bassembler_cfg_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bassembler.json");
        std::fs::write(&path, json).unwrap();
        path
    }

    const FULL: &str =
        r#"{"base":"v1","target":"t","projects":["../a","../b"],"issues":["A-1","B-2"]}"#;

    #[test]
    fn file_only() {
        let path = write_config("file_only", FULL);
        let cfg = load(Some(&path), Overrides::default()).unwrap();
        assert_eq!((cfg.base.as_str(), cfg.target.as_str()), ("v1", "t"));
        assert_eq!(cfg.issues, ["A-1", "B-2"]);
        assert_eq!(cfg.projects[0].dir, path.parent().unwrap().join("../a"));
    }

    #[test]
    fn command_line_overrides_file() {
        let path = write_config("override", FULL);
        let over = Overrides {
            base: Some("v2".into()),
            target: Some("t2".into()),
            projects: Some(vec!["x".into()]),
            issues: Some(IssuesSpec::List(vec!["C-3".into()])),
            ..Overrides::default()
        };
        let cfg = load(Some(&path), over).unwrap();
        assert_eq!((cfg.base.as_str(), cfg.target.as_str()), ("v2", "t2"));
        assert_eq!(cfg.issues, ["C-3"]);
        assert_eq!(cfg.projects.len(), 1);
        // command-line paths are relative to the current directory, not to the config file
        assert_eq!(cfg.projects[0].dir, PathBuf::from("./x"));
    }

    #[test]
    fn partial_override_keeps_the_rest() {
        let path = write_config("partial", FULL);
        let over = Overrides {
            target: Some("t2".into()),
            ..Overrides::default()
        };
        let cfg = load(Some(&path), over).unwrap();
        assert_eq!((cfg.base.as_str(), cfg.target.as_str()), ("v1", "t2"));
        assert_eq!(cfg.projects.len(), 2);
    }

    #[test]
    fn file_may_omit_what_the_command_line_gives() {
        let path = write_config("omit", r#"{"base":"v1"}"#);
        let over = Overrides {
            target: Some("t".into()),
            projects: Some(vec!["p".into()]),
            issues: Some(IssuesSpec::List(vec!["A-1".into()])),
            ..Overrides::default()
        };
        assert!(load(Some(&path), over).is_ok());
    }

    #[test]
    fn missing_value_names_both_sources() {
        let path = write_config("missing", r#"{"base":"v1"}"#);
        let err = load(Some(&path), Overrides::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("'target'") && err.contains("--target"),
            "{err}"
        );
    }

    #[test]
    fn explicit_missing_config_is_an_error() {
        let missing = std::env::temp_dir()
            .join("bassembler_no_such_dir")
            .join("x.json");
        assert!(load(Some(&missing), Overrides::default()).is_err());
    }

    #[test]
    fn resolver_and_conflict_from_file_and_override() {
        let json = r#"{"base":"v1","target":"t","projects":["p"],"issues":["A-1"],
                       "resolver":"tools/fix.bat","conflict":"skip"}"#;
        let path = write_config("resolver", json);
        let cfg = load(Some(&path), Overrides::default()).unwrap();
        assert_eq!(cfg.conflict, ConflictMode::Skip);
        assert_eq!(
            cfg.resolver.unwrap(),
            path.parent().unwrap().join("tools/fix.bat")
        );

        let over = Overrides {
            resolver: Some(PathBuf::from("mine.bat")),
            conflict: Some(ConflictMode::Abort),
            ..Overrides::default()
        };
        let cfg = load(Some(&path), over).unwrap();
        assert_eq!(cfg.conflict, ConflictMode::Abort);
        assert_eq!(cfg.resolver.unwrap(), PathBuf::from("mine.bat"));
    }

    #[test]
    fn resolver_and_conflict_default_to_none_and_abort() {
        let path = write_config("defaults", FULL);
        let cfg = load(Some(&path), Overrides::default()).unwrap();
        assert!(cfg.resolver.is_none());
        assert_eq!(cfg.conflict, ConflictMode::Abort);
    }

    #[test]
    fn bare_resolver_name_is_left_for_path_lookup() {
        let json = r#"{"base":"v1","target":"t","projects":["p"],"issues":["A-1"],"resolver":"nosuchtool"}"#;
        let path = write_config("bare", json);
        let cfg = load(Some(&path), Overrides::default()).unwrap();
        assert_eq!(cfg.resolver.unwrap(), PathBuf::from("nosuchtool"));
    }

    #[test]
    fn bad_conflict_value_is_rejected() {
        let json =
            r#"{"base":"v1","target":"t","projects":["p"],"issues":["A-1"],"conflict":"explode"}"#;
        let path = write_config("badconflict", json);
        assert!(load(Some(&path), Overrides::default()).is_err());
    }
}
