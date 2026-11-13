//! What `mods/` holds, compared to the manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use super::MODS_DIR;
use super::hash::{JarHashes, hash_file};
use crate::error::{Context, Result};
use crate::model::manifest::Manifest;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Keys of the entries whose jar is there with the manifest's hashes.
    pub present: Vec<String>,
    /// Keys of the entries whose jar is absent.
    pub missing: Vec<String>,
    /// Keys of the entries whose jar is there with other hashes.
    pub mismatched: Vec<String>,
    /// Jars of `mods/` no entry lists, as `mods/<name>`.
    pub unknown: Vec<String>,
    /// Every jar found, as `mods/<name>`, with its hashes.
    pub jars: BTreeMap<String, JarHashes>,
}

/// Hashes every jar of `<root>/mods/` and sorts the manifest's entries by
/// what was found. A missing `mods/` is an empty stock.
pub fn inventory(root: &Path, manifest: &Manifest) -> Result<Inventory> {
    let dir = root.join(MODS_DIR);
    let mut jars = BTreeMap::new();
    let listing = match fs::read_dir(&dir) {
        Ok(listing) => Some(listing),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context(format!("reading {}", dir.display())),
    };
    for item in listing.into_iter().flatten() {
        let item = item.context(format!("reading {}", dir.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().ends_with(".jar") || !item.path().is_file() {
            continue;
        }
        let hashes =
            hash_file(&item.path()).context(format!("reading {}", item.path().display()))?;
        jars.insert(format!("{MODS_DIR}/{name}"), hashes);
    }

    let mut inv = Inventory::default();
    for entry in &manifest.mods {
        let list = match jars.get(&entry.path) {
            None => &mut inv.missing,
            Some(found) if found.matches(&entry.hashes) => &mut inv.present,
            Some(_) => &mut inv.mismatched,
        };
        list.push(entry.key.clone());
    }
    let listed: BTreeSet<&str> = manifest.mods.iter().map(|e| e.path.as_str()).collect();
    inv.unknown = jars
        .keys()
        .filter(|path| !listed.contains(path.as_str()))
        .cloned()
        .collect();
    inv.jars = jars;
    Ok(inv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::download::tests::{entry, manifest};

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
    fn missing_mods_dir_is_an_empty_stock() {
        let root = tempfile::tempdir().unwrap();
        let inv = inventory(root.path(), &manifest(vec![entry("jei", b"jei")])).unwrap();
        assert_eq!(inv.missing, ["jei"]);
        assert!(inv.jars.is_empty());
    }
}
