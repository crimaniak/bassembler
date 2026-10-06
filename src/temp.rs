//! Names of the temporary branches a run works on, so an interrupted run can be continued.

use crate::config::Config;

const PREFIX: &str = "temp_";
const HASH_DIGITS: usize = 12;

/// `temp_<base>_<target>_<hash of the issue list>`. The same configuration always gives the same
/// name, so a second run finds the branch the first one left behind. The hash covers the issues
/// in order, since the order decides how the commits end up on the branch.
pub fn name(cfg: &Config) -> String {
    format!(
        "{PREFIX}{}_{}_{}",
        sanitize(&cfg.base),
        sanitize(&cfg.target),
        issues_hash(&cfg.issues)
    )
}

/// True for any branch named like [`name`] produces (a temporary branch of some run).
pub fn is_temp_branch(branch: &str) -> bool {
    let Some(rest) = branch.strip_prefix(PREFIX) else {
        return false;
    };
    rest.rsplit_once('_').is_some_and(|(_, hash)| {
        hash.len() == HASH_DIGITS && hash.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Keeps ASCII letters, digits, `.`, `_` and `-`; everything else (including `/`) becomes `-`.
/// A run of dots is cut to one because `..` is not allowed in a branch name.
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let ch = if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if ch == '.' && out.ends_with('.') {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    out
}

fn issues_hash(issues: &[String]) -> String {
    format!("{:016x}", fnv1a(issues.join("\n").as_bytes()))[..HASH_DIGITS].to_string()
}

/// FNV-1a, 64 bit. Written out because the standard hasher is not guaranteed to give the same
/// result across Rust versions, and the name has to stay the same between runs.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::ConflictMode;
    use crate::config::Config;

    fn cfg(base: &str, target: &str, issues: &[&str]) -> Config {
        Config {
            base: base.into(),
            target: target.into(),
            projects: Vec::new(),
            issues: issues.iter().map(|s| s.to_string()).collect(),
            resolver: None,
            conflict: ConflictMode::Abort,
        }
    }

    #[test]
    fn fnv_known_values() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn name_is_stable_and_depends_on_every_input() {
        let a = name(&cfg("v1.3.0", "out", &["A-1", "B-2"]));
        assert_eq!(a, name(&cfg("v1.3.0", "out", &["A-1", "B-2"])));
        assert!(
            a.starts_with("temp_v1.3.0_out_") && is_temp_branch(&a),
            "{a}"
        );
        assert_ne!(
            a,
            name(&cfg("v1.3.0", "out", &["B-2", "A-1"])),
            "order matters"
        );
        assert_ne!(a, name(&cfg("v1.3.0", "out", &["A-1"])));
        assert_ne!(a, name(&cfg("v1.3.1", "out", &["A-1", "B-2"])));
        assert_ne!(a, name(&cfg("v1.3.0", "out2", &["A-1", "B-2"])));
    }

    #[test]
    fn odd_names_are_made_safe() {
        let n = name(&cfg("release/1.4", "feat/x y", &["A-1"]));
        assert!(n.starts_with("temp_release-1.4_feat-x-y_"), "{n}");
        assert!(!name(&cfg("a..b", "t", &["A-1"])).contains(".."));
    }

    #[test]
    fn temp_branch_detection() {
        assert!(is_temp_branch("temp_v1_out_0123456789ab"));
        assert!(!is_temp_branch("temp_feature"));
        assert!(!is_temp_branch("temp_v1_out_0123456789xz"));
        assert!(!is_temp_branch("my_temp_v1_out_0123456789ab"));
        assert!(!is_temp_branch("temp_v1_out_0123456789a"));
    }
}
