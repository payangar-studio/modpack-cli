//! What the repository's kind directories and their local directories hold,
//! compared to the manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::hash::JarHashes;
use super::stock;
use crate::error::Result;
use crate::model::installed::Installed;
use crate::model::manifest::Manifest;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Keys of the entries whose file is there with the manifest's hashes.
    pub present: Vec<String>,
    /// Keys of the entries whose file is absent, or replaced by an older file
    /// the CLI downloaded and nobody changed since.
    pub missing: Vec<String>,
    /// Keys of the entries whose file is there with other hashes.
    pub mismatched: Vec<String>,
    /// Files no entry lists, as `<dir>/<name>` (`mods/<name>`,
    /// `resourcepacks-local/<name>`...).
    pub unknown: Vec<String>,
    /// Files the CLI downloaded that no entry lists any more, unchanged since,
    /// as `<kind dir>/<name>`: stale copies, never the user's.
    pub stale: Vec<String>,
    /// Every file found, as `<dir>/<name>`, with its hashes.
    pub jars: BTreeMap<String, JarHashes>,
}

/// Hashes every file of the repository's kind directories and local
/// directories (see [`stock`]) and sorts the manifest's entries by what was
/// found, telling the files the CLI downloaded (see [`Installed`]) from the
/// user's. A missing directory is an empty stock. The CLI never downloads
/// into a local directory: its files are the user's.
pub fn inventory(root: &Path, manifest: &Manifest) -> Result<Inventory> {
    let jars = stock(root)?;
    let installed = Installed::load(root)?;
    let placed = |path: &str, found: &JarHashes| installed.placed(path, &found.sha1);

    let mut inv = Inventory::default();
    for entry in &manifest.entries {
        let list = match jars.get(&entry.path) {
            None => &mut inv.missing,
            Some(found) if found.matches(&entry.hashes) => &mut inv.present,
            Some(found) if placed(&entry.path, found) => &mut inv.missing,
            Some(_) => &mut inv.mismatched,
        };
        list.push(entry.key.clone());
    }
    let listed: BTreeSet<&str> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
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
    fn resource_packs_are_sorted_like_mods() {
        let root = tempfile::tempdir().unwrap();
        for dir in ["resourcepacks", "resourcepacks-local"] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        fs::write(root.path().join("resourcepacks/faithful.zip"), b"faithful").unwrap();
        fs::write(root.path().join("resourcepacks/dropped.zip"), b"x").unwrap();
        fs::write(root.path().join("resourcepacks-local/house.zip"), b"house").unwrap();
        let mut faithful = entry("faithful", b"faithful");
        faithful.kind = crate::model::manifest::Kind::Resourcepack;
        faithful.path = "resourcepacks/faithful.zip".into();
        let mut house = entry("house", b"house");
        house.kind = crate::model::manifest::Kind::Resourcepack;
        house.path = "resourcepacks-local/house.zip".into();
        house.source = Some(crate::model::manifest::Source::Local);

        let inv = inventory(root.path(), &manifest(vec![faithful, house])).unwrap();
        assert_eq!(inv.present, ["faithful", "house"]);
        assert_eq!(inv.unknown, ["resourcepacks/dropped.zip"]);
    }

    #[test]
    fn missing_mods_dir_is_an_empty_stock() {
        let root = tempfile::tempdir().unwrap();
        let inv = inventory(root.path(), &manifest(vec![entry("jei", b"jei")])).unwrap();
        assert_eq!(inv.missing, ["jei"]);
        assert!(inv.jars.is_empty());
    }
}
