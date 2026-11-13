//! The repository's `mods/` directory seen as a stock of jars: what is there,
//! how it compares to the manifest, and how to fetch what is missing.

pub mod download;
pub mod hash;
pub mod inventory;

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::manifest::{self, Entry};

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
