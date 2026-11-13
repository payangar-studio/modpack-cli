//! The repository's `mods/` directory seen as a stock of jars: what is there,
//! how it compares to the manifest, and how to fetch what is missing.

pub mod download;
pub mod hash;
pub mod inventory;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Context, Error, Result};
use crate::model::manifest::{self, Entry};
use hash::{JarHashes, hash_file};

/// Directory of the jars, relative to the repository root.
pub const MODS_DIR: &str = "mods";

/// Where the jar of an entry lives under `root`.
///
/// The manifest is generated but stays a file anyone can edit: a path that is
/// not a plain file name directly under `mods/` is refused rather than written.
pub fn jar_path(root: &Path, entry: &Entry) -> Result<PathBuf> {
    let name = entry
        .path
        .strip_prefix(MODS_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|name| is_plain_file_name(name))
        .ok_or_else(|| {
            Error::usage(
                format!(
                    "{} gives `{}` the path `{}`, which is not a file directly under {MODS_DIR}/",
                    manifest::FILE_NAME,
                    entry.key,
                    entry.path
                ),
                format!("regenerate {} with `modpack manifest`", manifest::FILE_NAME),
            )
        })?;
    Ok(root.join(MODS_DIR).join(name))
}

/// Every `.jar` file directly under `dir`, a `mods/` directory, as
/// `mods/<name>` with its hashes. A missing directory holds none.
pub fn jars_in(dir: &Path) -> Result<BTreeMap<String, JarHashes>> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).context(format!("reading {}", dir.display())),
    };
    let mut jars = BTreeMap::new();
    for item in listing {
        let item = item.context(format!("reading {}", dir.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !name.to_ascii_lowercase().ends_with(".jar") || !item.path().is_file() {
            continue;
        }
        let hashes =
            hash_file(&item.path()).context(format!("reading {}", item.path().display()))?;
        jars.insert(format!("{MODS_DIR}/{name}"), hashes);
    }
    Ok(jars)
}

/// Writes `dest` through a temporary file next to it, filled by `fill`, then
/// renamed: a crash, or a `fill` that fails, never leaves a partial file
/// under the final name.
pub fn write_atomic<T>(dest: &Path, fill: impl FnOnce(&mut fs::File) -> Result<T>) -> Result<T> {
    let dir = dest.parent().expect("a destination has a parent directory");
    fs::create_dir_all(dir).context(format!("creating {}", dir.display()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".")
        .suffix(".part")
        .tempfile_in(dir)
        .context(format!("creating a temporary file in {}", dir.display()))?;
    let value = fill(tmp.as_file_mut())?;
    tmp.as_file()
        .sync_all()
        .context(format!("writing {}", dest.display()))?;
    tmp.persist(dest)
        .map_err(|e| e.error)
        .context(format!("writing {}", dest.display()))?;
    Ok(value)
}

fn is_plain_file_name(name: &str) -> bool {
    !name.contains(['/', '\\'])
        && matches!(
            Path::new(name).components().collect::<Vec<_>>()[..],
            [Component::Normal(_)]
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_path_accepts_only_files_directly_under_mods() {
        let root = Path::new("/repo");
        let mut entry = download::tests::entry("jei", b"jei");
        assert_eq!(
            jar_path(root, &entry).unwrap(),
            root.join("mods").join("jei-1.0.0.jar")
        );
        for bad in [
            "mods/../x.jar",
            "mods/a/b.jar",
            "mods/",
            "x.jar",
            "/mods/x.jar",
            "mods/..",
            "mods\\x.jar",
        ] {
            entry.path = bad.into();
            assert!(jar_path(root, &entry).is_err(), "{bad} accepted");
        }
    }
}
