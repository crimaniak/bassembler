//! Checks of the environment that don't belong to a single project.

use std::process::Command;

/// Oldest git that has everything bassembler uses (`cherry-pick --skip` came in 2.23).
const MIN_GIT: (u32, u32) = (2, 23);

/// Checks that `git` can be run and is new enough.
pub fn check_git() -> Result<(), String> {
    let out = Command::new("git").arg("--version").output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "git was not found on PATH".to_string()
        } else {
            format!("cannot run git: {e}")
        }
    })?;
    if !out.status.success() {
        return Err("'git --version' failed".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    match parse_git_version(&text) {
        Some(v) if v < MIN_GIT => Err(format!(
            "git {}.{} is too old, {}.{} or newer is required",
            v.0, v.1, MIN_GIT.0, MIN_GIT.1
        )),
        _ => Ok(()),
    }
}

/// `git version 2.43.0.windows.1` -> (2, 43). None if the output isn't recognised, in which
/// case the version check is skipped rather than failing a working git.
fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let version = text.trim().strip_prefix("git version ")?;
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::parse_git_version;

    #[test]
    fn git_version_parsing() {
        assert_eq!(
            parse_git_version("git version 2.43.0.windows.1\n"),
            Some((2, 43))
        );
        assert_eq!(parse_git_version("git version 2.34.1"), Some((2, 34)));
        assert_eq!(parse_git_version("git version 2.20.0"), Some((2, 20)));
        assert_eq!(parse_git_version("something else"), None);
    }
}
