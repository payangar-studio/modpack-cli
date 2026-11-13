//! Local mods: jars no platform hosts, tracked by git in `mods-local/`, and
//! their approval for publication.
//!
//! `modpack approve <key>` records in the manifest the sha512 of the jar it
//! approves. The approval holds while the jar keeps that hash: a changed jar
//! is rebuilt by `modpack manifest` without it. The publish workflow refuses
//! to publish while a local mod is not approved.

use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::error::{Context, Error, Result};
use crate::model::manifest::{self, Entry, Manifest};
use crate::store::hash::hash_file;
use crate::store::jar_path;

/// A local mod of the manifest, as `status` and `pack` report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalMod {
    pub key: String,
    /// `mods-local/<name>`.
    pub path: String,
    /// Whether the approval in the manifest matches the jar in `mods-local/`.
    pub approved: bool,
}

impl LocalMod {
    /// The line that says what to do about an unapproved local mod.
    pub fn todo(&self) -> String {
        format!(
            "`{}` ({}) is not approved for publication: review the jar, then run `modpack approve {}` and commit {}",
            self.key,
            self.path,
            self.key,
            manifest::FILE_NAME
        )
    }
}

/// The local mods of the manifest, sorted by key. A mod is approved when its
/// approval matches the manifest's hash and the jar in `mods-local/` still has
/// it. Reads files only.
pub fn local_mods(root: &Path, manifest: &Manifest) -> Result<Vec<LocalMod>> {
    let mut mods = Vec::new();
    for entry in manifest.mods.iter().filter(|e| e.is_local()) {
        mods.push(LocalMod {
            key: entry.key.clone(),
            path: entry.path.clone(),
            approved: entry.is_approved() && jar_matches(root, entry)?,
        });
    }
    mods.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(mods)
}

/// Whether the jar of a local mod is in `mods-local/` with the manifest's
/// sha512.
fn jar_matches(root: &Path, entry: &Entry) -> Result<bool> {
    let jar = jar_path(root, entry)?;
    if !jar.is_file() {
        return Ok(false);
    }
    let hashes = hash_file(&jar).context(format!("reading {}", jar.display()))?;
    Ok(hashes.sha512 == entry.hashes.sha512)
}

/// Approves the local mod `key`: records the sha512 of its jar in the
/// manifest.
pub fn approve(root: &Path, key: &str, out: &mut dyn Write) -> Result<()> {
    let path = root.join(manifest::FILE_NAME);
    let mut manifest = Manifest::load(&path)?;
    let entry = manifest.get_mut(key).ok_or_else(|| {
        Error::usage(
            format!("no mod `{key}` in {}", manifest::FILE_NAME),
            "use the key of a local mod, as `modpack status` lists them",
        )
    })?;
    if !entry.is_local() {
        return Err(Error::usage(
            format!("`{key}` is not a local mod: a platform or a URL serves it"),
            "only the jars of mods-local/ need an approval",
        ));
    }
    let jar = jar_path(root, entry)?;
    if !jar.is_file() {
        return Err(Error::usage(
            format!("the jar of `{key}`, {}, is missing", entry.path),
            format!("restore it with `git checkout -- {}`", entry.path),
        ));
    }
    let sha512 = hash_file(&jar)
        .context(format!("reading {}", jar.display()))?
        .sha512;
    if sha512 != entry.hashes.sha512 {
        return Err(Error::usage(
            format!(
                "{} changed since {} was generated",
                entry.path,
                manifest::FILE_NAME
            ),
            "run `modpack manifest`, review the new jar, then approve it",
        ));
    }
    if entry.approved.as_deref() == Some(sha512.as_str()) {
        writeln!(out, "`{key}` is already approved")?;
        return Ok(());
    }
    let rel = entry.path.clone();
    entry.approved = Some(sha512.clone());
    manifest.save(&path)?;
    writeln!(
        out,
        "approved `{key}` ({rel}, sha512 {}…); commit {}",
        &sha512[..16],
        manifest::FILE_NAME
    )?;
    Ok(())
}

/// Fails with exit code 3 while a local mod is not approved, writing one line
/// per such mod to `out`. The publication gate of the CI.
pub fn check_approved(root: &Path, out: &mut dyn Write) -> Result<()> {
    let manifest = Manifest::load(&root.join(manifest::FILE_NAME))?;
    let mods = local_mods(root, &manifest)?;
    let pending: Vec<&LocalMod> = mods.iter().filter(|m| !m.approved).collect();
    if pending.is_empty() {
        match mods.len() {
            0 => writeln!(out, "no local mod")?,
            n => writeln!(out, "{n} local mod(s), all approved")?,
        }
        return Ok(());
    }
    let mut what = format!(
        "{} local mod(s) not approved for publication:",
        pending.len()
    );
    for m in &pending {
        // Not approved because the jar is absent, as in a checkout where it
        // was never committed: approving would not help.
        let missing = manifest
            .get(&m.key)
            .map(|e| jar_path(root, e))
            .transpose()?
            .is_some_and(|jar| !jar.is_file());
        if missing {
            writeln!(
                out,
                "`{}` ({}) has no jar in the repository: commit it, or take the mod out with `modpack remove {}`",
                m.key, m.path, m.key
            )?;
        } else {
            writeln!(out, "{}", m.todo())?;
        }
        let _ = write!(what, "\n  - {} ({})", m.key, m.path);
    }
    Err(Error::blocked(
        what,
        "review each jar, then run `modpack approve <key>` and commit mods.lock.toml",
    ))
}

/// What to do about local mods whose jar is missing from `mods-local/` or
/// changed: git holds those jars, nothing downloads them, so
/// `modpack install` cannot help.
pub fn restore_hint(entries: &[&Entry]) -> String {
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    match entries {
        [entry] => format!(
            "restore it from git with `git checkout -- {}`, or take the mod out with `modpack remove {}`",
            entry.path, entry.key
        ),
        _ => format!(
            "restore them from git with `git checkout -- {}`, or take each mod out with `modpack remove <key>`",
            paths.join(" ")
        ),
    }
}

/// Whether git holds the jar of a local mod as committed: tracked, and the
/// same in the working tree, the index and `HEAD`. Outside a git repository,
/// or when git cannot run, it is not.
pub fn jar_committed(root: &Path, entry: &Entry) -> bool {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };
    git(&["ls-files", "--error-unmatch", "--", &entry.path]).is_some()
        && git(&["status", "--porcelain", "--", &entry.path]).is_some_and(|o| o.is_empty())
}

/// Removes the jar of a local mod from the working tree and from git, or from
/// the working tree only when git does not track it.
pub fn remove_jar(root: &Path, entry: &Entry) -> Result<()> {
    let jar = jar_path(root, entry)?;
    if !jar.is_file() {
        return Ok(());
    }
    let removed = Command::new("git")
        .args(["rm", "--quiet", "--force", "--", &entry.path])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !removed {
        fs::remove_file(&jar).context(format!("removing {}", jar.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::ops::manifest::regenerate;
    use crate::ops::manifest::tests::{registry, repo};

    fn load(root: &Path) -> Manifest {
        Manifest::load(&root.join(manifest::FILE_NAME)).unwrap()
    }

    /// A repository with the local mod `custom`, in the manifest.
    fn with_local() -> tempfile::TempDir {
        let root = repo();
        fs::create_dir_all(root.path().join("mods-local")).unwrap();
        fs::write(root.path().join("mods-local/custom.jar"), b"v1").unwrap();
        regenerate(root.path(), &registry()).unwrap();
        root
    }

    fn approve_out(root: &Path, key: &str) -> Result<String> {
        let mut out = Vec::new();
        approve(root, key, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn approve_records_the_hash_of_the_jar() {
        let root = with_local();
        assert!(!local_mods(root.path(), &load(root.path())).unwrap()[0].approved);

        let out = approve_out(root.path(), "custom").unwrap();
        assert!(out.starts_with("approved `custom`"), "{out}");
        let custom = load(root.path()).get("custom").unwrap().clone();
        assert_eq!(
            custom.approved.as_deref(),
            Some(custom.hashes.sha512.as_str())
        );
        assert_eq!(
            local_mods(root.path(), &load(root.path())).unwrap(),
            [LocalMod {
                key: "custom".into(),
                path: "mods-local/custom.jar".into(),
                approved: true
            }]
        );
        let out = approve_out(root.path(), "custom").unwrap();
        assert!(out.contains("already approved"), "{out}");
    }

    #[test]
    fn a_changed_jar_is_not_approved_and_cannot_be_until_regenerated() {
        let root = with_local();
        approve_out(root.path(), "custom").unwrap();
        fs::write(root.path().join("mods-local/custom.jar"), b"v2").unwrap();

        // Before `modpack manifest`: the jar no longer has the approved hash.
        assert!(!local_mods(root.path(), &load(root.path())).unwrap()[0].approved);
        let err = approve_out(root.path(), "custom").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.hint().contains("modpack manifest"), "{}", err.hint());

        // After: rebuilt without approval, then approved again.
        regenerate(root.path(), &registry()).unwrap();
        assert!(!local_mods(root.path(), &load(root.path())).unwrap()[0].approved);
        approve_out(root.path(), "custom").unwrap();
        assert!(local_mods(root.path(), &load(root.path())).unwrap()[0].approved);
    }

    #[test]
    fn only_a_local_mod_of_the_manifest_is_approved() {
        let root = with_local();
        fs::write(root.path().join("mods/sodium-1.0.0.jar"), b"sodium").unwrap();
        regenerate(root.path(), &registry()).unwrap();
        let err = approve_out(root.path(), "sodium").unwrap_err();
        assert!(err.to_string().contains("not a local mod"), "{err}");
        let err = approve_out(root.path(), "nope").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
    }

    #[test]
    fn the_gate_names_each_unapproved_mod_and_the_command() {
        let root = with_local();
        let mut out = Vec::new();
        let err = check_approved(root.path(), &mut out).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Blocked);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("`custom` (mods-local/custom.jar)"), "{out}");
        assert!(out.contains("`modpack approve custom`"), "{out}");

        approve_out(root.path(), "custom").unwrap();
        let mut out = Vec::new();
        check_approved(root.path(), &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "1 local mod(s), all approved\n"
        );

        // A jar absent from the checkout is named as such, not as unreviewed.
        fs::remove_file(root.path().join("mods-local/custom.jar")).unwrap();
        let mut out = Vec::new();
        check_approved(root.path(), &mut out).unwrap_err();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("has no jar in the repository"), "{out}");
    }

    #[test]
    fn remove_jar_outside_git_deletes_the_file() {
        let root = with_local();
        let custom = load(root.path()).get("custom").unwrap().clone();
        remove_jar(root.path(), &custom).unwrap();
        assert!(!root.path().join("mods-local/custom.jar").exists());
        remove_jar(root.path(), &custom).unwrap();
    }
}
