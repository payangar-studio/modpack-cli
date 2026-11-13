//! The version a pack is built with.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

/// Version of a pack built neither with `--pack-version` nor on a tag.
pub const DEV_VERSION: &str = "0.0.0-dev";

/// `explicit` when given, else the git tag on the commit checked out at `root`
/// without its leading `v` (`v1.2.3` gives `1.2.3`), else [`DEV_VERSION`].
pub fn version(root: &Path, explicit: Option<&str>) -> Result<String> {
    let version = match explicit {
        Some(v) => v.trim().to_owned(),
        None => tag_version(root).unwrap_or_else(|| DEV_VERSION.to_owned()),
    };
    if version.is_empty() || version.contains(['/', '\\']) {
        return Err(Error::usage(
            format!("`{version}` cannot be a pack version: it goes into file names"),
            "pass a version without slashes with --pack-version, such as 1.2.3",
        ));
    }
    Ok(version)
}

/// The tag on HEAD, `None` when there is none or git cannot tell.
fn tag_version(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--exact-match", "HEAD"])
        .current_dir(root)
        .output()
        .inspect_err(|e| tracing::debug!("could not run git to read the tag: {e}"))
        .ok()?;
    if !output.status.success() {
        tracing::debug!("no git tag on HEAD");
        return None;
    }
    let tag = String::from_utf8(output.stdout).ok()?;
    let tag = tag.trim();
    let version = match tag.strip_prefix('v') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => tag,
    };
    Some(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    /// Runs git in `dir`, isolated from the machine's git configuration.
    fn git(dir: &Path, args: &[&str]) {
        let empty = dir.join(".gitconfig-test");
        std::fs::write(&empty, "").unwrap();
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.org",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", &empty)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn explicit_version_wins() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(version(dir.path(), Some(" 2.0.0 ")).unwrap(), "2.0.0");
    }

    #[test]
    fn version_comes_from_the_tag_on_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(
            dir.path(),
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
        assert_eq!(version(dir.path(), None).unwrap(), DEV_VERSION, "no tag");

        git(dir.path(), &["tag", "v1.2.3"]);
        assert_eq!(version(dir.path(), None).unwrap(), "1.2.3");

        git(
            dir.path(),
            &["commit", "--quiet", "--allow-empty", "-m", "second"],
        );
        assert_eq!(
            version(dir.path(), None).unwrap(),
            DEV_VERSION,
            "a tag on an older commit is not the current one"
        );

        git(dir.path(), &["tag", "beta"]);
        assert_eq!(version(dir.path(), None).unwrap(), "beta");
    }

    #[test]
    fn outside_git_the_version_is_dev() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(version(dir.path(), None).unwrap(), DEV_VERSION);
    }

    #[test]
    fn a_version_that_is_not_a_file_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "../1.0", "a\\b"] {
            let err = version(dir.path(), Some(bad)).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Usage, "{bad}");
        }
    }
}
