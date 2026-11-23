//! The build's version, as data and as a label.
//!
//! [`VERSION`] is the crate version, `0.11.0`: what the update check
//! compares and what sidecars, exports, and caches record. [`LABEL`]
//! is what a person reads (the log's first line, `--version`, About,
//! a problem report): the same for a release, and for any other build
//! from a git checkout the commit after it, `0.11.0 (79527a9)`, with a
//! `+` when tracked files had changes at the first build after HEAD
//! last moved (a later uncommitted edit does not add it; the stamp
//! reruns only when HEAD or the tags move). Past the release's tag
//! `v<version>` the count of commits since it follows the version,
//! `0.11.0+135 (79527a9)`, so a build from master reads as newer than
//! the release; where git cannot count (no tag among HEAD's ancestors,
//! a shallow clone) the count is left out. The build script stamps it;
//! a packager may set `GREYCARD_COMMIT` at build time instead, which
//! leaves the count out, and the release workflow sets
//! `GREYCARD_RELEASE`. [`KEY`] is the label without the count, for a
//! record that names the build. Kept in a crate of its own so a new
//! commit rebuilds only it and the two binaries' crates.

// The rest of it is the build script's, tested here.
#[allow(dead_code)]
mod stamp;

pub use stamp::label;

/// The crate version, for comparing and recording.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit part of the label, empty for a release or a build with
/// no git.
pub const COMMIT: &str = env!("GREYCARD_COMMIT");

/// The version as shown to a person.
pub const LABEL: &str = env!("GREYCARD_LABEL");

/// The label without the count, `0.11.0 (79527a9)`: what a record keys
/// a build on, so the same commit stays the same build when a tag is
/// fetched later or a shallow clone is deepened.
pub const KEY: &str = env!("GREYCARD_KEY");

#[cfg(test)]
mod tests {
    use super::stamp::*;
    use super::*;

    #[test]
    fn a_release_is_the_plain_version() {
        assert_eq!(
            label("0.11.0", 0, &commit(true, Some("79527a9"), false)),
            "0.11.0"
        );
    }

    #[test]
    fn a_release_said_by_the_workflow_ignores_changes() {
        assert_eq!(commit(true, Some("79527a9"), true), "");
        assert_eq!(commit(true, None, false), "");
    }

    #[test]
    fn another_build_carries_its_commit() {
        assert_eq!(
            label("0.11.0", 0, &commit(false, Some("79527a9"), false)),
            "0.11.0 (79527a9)"
        );
    }

    #[test]
    fn uncommitted_changes_add_a_plus() {
        assert_eq!(
            label("0.11.0", 0, &commit(false, Some("79527a9"), true)),
            "0.11.0 (79527a9+)"
        );
    }

    #[test]
    fn no_git_is_the_plain_version() {
        assert_eq!(label("0.11.0", 0, &commit(false, None, true)), "0.11.0");
    }

    #[test]
    fn git_output_parses() {
        assert_eq!(hash("79527a9\n"), Some("79527a9"));
        assert_eq!(hash("fatal: not a git repository\n"), None);
        assert_eq!(hash(""), None);
        assert_eq!(hash("79527"), None);
        assert!(is_release_tag("v0.11.0\n", "0.11.0"));
        assert!(!is_release_tag("v0.11.0-1-g79527a9\n", "0.11.0"));
        assert!(!is_release_tag("v0.10.0\n", "0.11.0"));
        assert!(!is_release_tag("", "0.11.0"));
        assert!(dirty(" M crates/greycard-ui/src/log.rs\n"));
        assert!(!dirty("\n"));
    }

    #[test]
    fn a_packagers_commit_is_kept_if_plain() {
        assert_eq!(overridden(" 79527a9 \n"), Some("79527a9"));
        assert_eq!(overridden(""), Some(""));
        assert_eq!(overridden("abc\ndef"), None);
        assert_eq!(overridden("a b"), None);
    }

    #[test]
    fn commits_past_the_tag_follow_the_version() {
        let c = commit(false, Some("79527a9"), false);
        assert_eq!(label("0.11.0", 135, &c), "0.11.0+135 (79527a9)");
        let dirty = commit(false, Some("79527a9"), true);
        assert_eq!(label("0.11.0", 1, &dirty), "0.11.0+1 (79527a9+)");
    }

    /// A release, a packager's empty commit, or no git: the count is
    /// never shown without a commit beside it.
    #[test]
    fn no_commit_shows_no_count() {
        assert_eq!(
            label("0.11.0", 135, &commit(true, Some("79527a9"), false)),
            "0.11.0"
        );
        assert_eq!(label("0.11.0", 135, ""), "0.11.0");
    }

    #[test]
    fn describe_output_parses_as_the_count() {
        assert_eq!(since("v0.11.0-135-g79527a9\n", "0.11.0"), Some(135));
        assert_eq!(since("v0.11.0-0-g79527a9c", "0.11.0"), Some(0));
        // Another version's tag, or one that only starts the same.
        assert_eq!(since("v0.10.0-135-g79527a9", "0.11.0"), None);
        assert_eq!(since("v0.11.0-rc1-3-g79527a9", "0.11.0"), None);
        assert_eq!(since("v0.1-3-g79527a9", "0.11.0"), None);
        assert_eq!(since("v0.11.01-3-g79527a9", "0.11.0"), None);
        // Not describe's long form, or git's complaint.
        assert_eq!(since("v0.11.0", "0.11.0"), None);
        assert_eq!(since("v0.11.0-g79527a9", "0.11.0"), None);
        assert_eq!(since("v0.11.0-+3-g79527a9", "0.11.0"), None);
        assert_eq!(since("v0.11.0-3-gzzzzzzz", "0.11.0"), None);
        assert_eq!(since("v0.11.0-99999999999-g79527a9", "0.11.0"), None);
        assert_eq!(since("fatal: No names found\n", "0.11.0"), None);
        assert_eq!(since("", "0.11.0"), None);
    }

    #[test]
    fn only_a_full_clone_counts() {
        assert!(!shallow("false\n"));
        assert!(shallow("true\n"));
        // A git too old to know the flag echoes it back.
        assert!(shallow("--is-shallow-repository\n"));
        assert!(shallow(""));
    }

    /// This build's own stamp follows the rules.
    #[test]
    fn the_label_is_the_version_and_the_commit() {
        let since = env!("GREYCARD_SINCE").parse().unwrap();
        assert_eq!(LABEL, label(VERSION, since, COMMIT));
        assert_eq!(KEY, label(VERSION, 0, COMMIT));
        assert!(LABEL.starts_with(VERSION));
    }
}
