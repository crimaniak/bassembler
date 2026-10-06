//! Running and locating the user's conflict-resolver program.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The command that runs `resolver`. Windows cannot execute a `.ps1` file directly, so there it
/// goes through PowerShell; everything else (and any file on other platforms) is run as is.
pub fn command(resolver: &Path) -> Command {
    if cfg!(windows) && is_powershell_script(resolver) {
        let host = powershell_host().unwrap_or_else(|| PathBuf::from("powershell.exe"));
        let mut command = Command::new(host);
        command.args(powershell_args(resolver));
        return command;
    }
    Command::new(resolver)
}

/// Checks, without running it, that `resolver` can be started: it exists, is a file, is
/// executable (Unix) and, for a Windows `.ps1`, that PowerShell is installed.
pub fn check(resolver: &Path) -> Result<(), String> {
    let shown = resolver.display();
    let found = locate(resolver)?;
    if found.is_dir() {
        return Err(format!("'{shown}' is a directory, not a program"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&found)
            .map(|m| m.permissions().mode())
            .unwrap_or(0);
        if mode & 0o111 == 0 {
            return Err(format!(
                "'{}' is not executable (chmod +x)",
                found.display()
            ));
        }
    }
    if cfg!(windows) && is_powershell_script(resolver) && powershell_host().is_none() {
        return Err(format!(
            "'{shown}' needs powershell.exe or pwsh.exe on PATH"
        ));
    }
    Ok(())
}

/// The file `resolver` refers to: a path as given, or a bare name looked up on `PATH`.
fn locate(resolver: &Path) -> Result<PathBuf, String> {
    let is_path = resolver.components().count() > 1 || resolver.exists();
    if is_path {
        return if resolver.exists() {
            Ok(resolver.to_path_buf())
        } else {
            Err(format!("'{}' not found", resolver.display()))
        };
    }
    // PowerShell resolves a bare script name against the project directory, not PATH.
    if cfg!(windows) && is_powershell_script(resolver) {
        return Err(format!("script '{}' not found", resolver.display()));
    }
    find_on_path(resolver).ok_or_else(|| format!("'{}' not found on PATH", resolver.display()))
}

fn find_on_path(name: &Path) -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH")?).collect();
    // Like Rust's own lookup, Windows also tries the name with `.exe` appended.
    let mut names = vec![name.to_path_buf()];
    if cfg!(windows) && name.extension().is_none() {
        names.push(name.with_extension("exe"));
    }
    names
        .iter()
        .find_map(|n| dirs.iter().map(|d| d.join(n)).find(|f| f.is_file()))
}

fn is_powershell_script(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ps1"))
}

/// Windows PowerShell if present (it ships with Windows), otherwise PowerShell 7.
fn powershell_host() -> Option<PathBuf> {
    ["powershell.exe", "pwsh.exe"]
        .iter()
        .find_map(|exe| find_on_path(Path::new(exe)))
}

fn powershell_args(script: &Path) -> Vec<OsString> {
    // -File makes the script's `exit N` the process exit code.
    [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]
    .iter()
    .map(OsString::from)
    .chain([script.as_os_str().to_owned()])
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bassembler_res_{}_{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn powershell_script_detection() {
        assert!(is_powershell_script(Path::new("fix.ps1")));
        assert!(is_powershell_script(Path::new("C:/tools/Fix.PS1")));
        assert!(!is_powershell_script(Path::new("fix.bat")));
        assert!(!is_powershell_script(Path::new("fix")));
    }

    #[test]
    fn powershell_runs_the_script_with_file() {
        let args = powershell_args(Path::new("a b/fix.ps1"));
        assert_eq!(args.last().unwrap(), "a b/fix.ps1");
        assert_eq!(args[args.len() - 2], "-File");
    }

    #[test]
    fn missing_resolver_is_reported() {
        let dir = scratch("missing");
        assert!(
            check(&dir.join("nope.sh"))
                .unwrap_err()
                .contains("not found")
        );
        assert!(
            check(Path::new("no_such_program_xyz"))
                .unwrap_err()
                .contains("on PATH")
        );
    }

    #[test]
    fn directory_is_not_a_resolver() {
        let dir = scratch("dir");
        assert!(check(&dir).unwrap_err().contains("directory"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_needs_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("exec");
        let f = dir.join("fix.sh");
        std::fs::write(&f, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check(&f).unwrap_err().contains("not executable"));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check(&f).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_files_pass() {
        let dir = scratch("win");
        for name in ["fix.bat", "fix.ps1"] {
            let f = dir.join(name);
            std::fs::write(&f, "exit 0\n").unwrap();
            assert!(check(&f).is_ok(), "{name}");
        }
    }
}
