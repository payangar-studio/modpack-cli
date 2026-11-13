//! What `mods/` and `mods-local/` hold, compared to the manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::hash::JarHashes;
use super::{MODS_DIR, jars_in, local_jars};
use crate::error::Result;
use crate::model::installed::Installed;
use crate::model::manifest::Manifest;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Keys of the entries whose jar is there with the manifest's hashes.
    pub present: Vec<String>,
    /// Keys of the entries whose jar is absent, or replaced by an older jar
    /// the CLI downloaded and nobody changed since.
    pub missing: Vec<String>,
    /// Keys of the entries whose jar is there with other hashes.
    pub mismatched: Vec<String>,
    /// Jars of `mods/` and `mods-local/` no entry lists, as `mods/<name>` or
    /// `mods-local/<name>`.
    pub unknown: Vec<String>,
    /// Jars the CLI downloaded that no entry lists any more, unchanged since,
    /// as `mods/<name>`: stale copies, never the user's.
    pub stale: Vec<String>,
    /// Every jar found, as `mods/<name>` or `mods-local/<name>`, with its
    /// hashes.
    pub jars: BTreeMap<String, JarHashes>,
}

/// Hashes every jar of `<root>/mods/` and `<root>/mods-local/` and sorts the
/// manifest's entries by what was found, telling the jars the CLI downloaded
/// (see [`Installed`]) from the user's. A missing directory is an empty
/// stock. The CLI never downloads into `mods-local/`: its jars are the user's.
pub fn inventory(root: &Path, manifest: &Manifest) -> Result<Inventory> {
    let mut jars = jars_in(&root.join(MODS_DIR))?;
    jars.extend(local_jars(root)?);
    let installed = Installed::load(root)?;
    let placed = |path: &str, found: &JarHashes| installed.placed(path, &found.sha1);

    let mut inv = Inventory::default();
    for entry in &manifest.mods {
        let list = match jars.get(&entry.path) {
            None => &mut inv.missing,
            Some(found) if found.matches(&entry.hashes) => &mut inv.present,
            Some(found) if placed(&entry.path, found) => &mut inv.missing,
            Some(_) => &mut inv.mismatched,
        };
        list.push(entry.key.clone());
    }
    let listed: BTreeSet<&str> = manifest.mods.iter().map(|e| e.path.as_str()).collect();
    for (path, found) in &jars {
        if listed.contains(path.as_str()) {
            continue;
        }
        let list = if placed(path, found) {
            &mut inv.stale
        } else {
            &mut inv.unknown
        };
        list.push(path.clone());
    }
    inv.jars = jars;
    Ok(inv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::download::tests::{entry, manifest};
    use std::fs;

    #[test]
    fn sorts_entries_by_what_mods_holds() {
        let root = tempfile::tempdir().unwrap();
        let mods = root.path().join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("jei-1.0.0.jar"), b"jei").unwrap();
        fs::write(mods.join("iris-1.0.0.jar"), b"tampered").unwrap();
        fs::write(mods.join("tried-in-the-app.jar"), b"x").unwrap();
        fs::write(mods.join("notes.txt"), b"not a jar").unwrap();
        let manifest = manifest(vec![
            entry("jei", b"jei"),
            entry("iris", b"iris"),
            entry("sodium", b"sodium"),
        ]);

        let inv = inventory(root.path(), &manifest).unwrap();
        assert_eq!(inv.present, ["jei"]);
        assert_eq!(inv.mismatched, ["iris"]);
        assert_eq!(inv.missing, ["sodium"]);
        assert_eq!(inv.unknown, ["mods/tried-in-the-app.jar"]);
        assert_eq!(inv.jars.len(), 3);
        assert_eq!(inv.jars["mods/jei-1.0.0.jar"].size, 3);
    }

    #[test]
    fn local_jars_are_compared_to_local_entries() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("mods-local");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("custom-1.0.0.jar"), b"custom").unwrap();
        fs::write(local.join("dropped.jar"), b"new").unwrap();
        let mut custom = entry("custom", b"custom");
        custom.path = "mods-local/custom-1.0.0.jar".into();
        custom.source = Some(crate::model::manifest::Source::Local);

        let inv = inventory(root.path(), &manifest(vec![custom])).unwrap();
        assert_eq!(inv.present, ["custom"]);
        assert_eq!(inv.unknown, ["mods-local/dropped.jar"]);
        assert!(inv.stale.is_empty());
    }

    #[test]
    fn missing_mods_dir_is_an_empty_stock() {
        let root = tempfile::tempdir().unwrap();
        let inv = inventory(root.path(), &manifest(vec![entry("jei", b"jei")])).unwrap();
        assert_eq!(inv.missing, ["jei"]);
        assert!(inv.jars.is_empty());
    }
}
