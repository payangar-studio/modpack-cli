//! The repository's kind directories (`mods/`, `resourcepacks/`) seen as a
//! stock of files: what is there, how it compares to the manifest, and how to
//! fetch what is missing. Local files sit apart, in each kind's local
//! directory (`mods-local/`, `resourcepacks-local/`), tracked by git.

pub mod download;
pub mod hash;
pub mod inventory;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Context, Error, Result};
use crate::model::manifest::{self, Entry, Kind};
use hash::{JarHashes, hash_file};

/// Where the file of an entry lives under `root`: its kind's directory, or
/// its kind's local directory for a local entry.
///
/// The manifest is generated but stays a file anyone can edit: a path that is
/// not a plain file name directly under that directory is refused rather than
/// written.
pub fn file_path(root: &Path, entry: &Entry) -> Result<PathBuf> {
    let dir = if entry.is_local() {
        entry.kind.local_dir()
    } else {
        entry.kind.dir()
    };
    let name = entry
        .path
        .strip_prefix(dir)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|name| is_plain_file_name(name))
        .ok_or_else(|| {
            Error::usage(
                format!(
                    "{} gives `{}` the path `{}`, which is not a file directly under {dir}/",
                    manifest::FILE_NAME,
                    entry.key,
                    entry.path
                ),
                format!("regenerate {} with `modpack manifest`", manifest::FILE_NAME),
            )
        })?;
    Ok(root.join(dir).join(name))
}

/// Every file of every kind under `root`, an instance or a server: the files
/// with the kind's extension directly under its directory, as
/// `<kind dir>/<name>` with their hashes. A missing directory holds none.
pub fn files_in(root: &Path) -> Result<BTreeMap<String, JarHashes>> {
    let mut files = BTreeMap::new();
    for kind in Kind::ALL {
        files.extend(list_files(&root.join(kind.dir()), kind.dir(), kind)?);
    }
    Ok(files)
}

/// Every file of the repository at `root`: [`files_in`], plus the files of
/// each kind's local directory, as `<local dir>/<name>`.
pub fn stock(root: &Path) -> Result<BTreeMap<String, JarHashes>> {
    let mut files = files_in(root)?;
    for kind in Kind::ALL {
        files.extend(list_files(
            &root.join(kind.local_dir()),
            kind.local_dir(),
            kind,
        )?);
    }
    Ok(files)
}

fn list_files(dir: &Path, prefix: &str, kind: Kind) -> Result<BTreeMap<String, JarHashes>> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).context(format!("reading {}", dir.display())),
    };
    let mut files = BTreeMap::new();
    for item in listing {
        let item = item.context(format!("reading {}", dir.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if Kind::of_file_name(&name) != Some(kind) || !item.path().is_file() {
            continue;
        }
        let hashes =
            hash_file(&item.path()).context(format!("reading {}", item.path().display()))?;
        files.insert(format!("{prefix}/{name}"), hashes);
    }
    Ok(files)
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
    fn file_path_accepts_only_files_directly_under_the_kind_dir() {
        let root = Path::new("/repo");
        let mut entry = download::tests::entry("jei", b"jei");
        assert_eq!(
            file_path(root, &entry).unwrap(),
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
            assert!(file_path(root, &entry).is_err(), "{bad} accepted");
        }

        entry.source = Some(crate::model::manifest::Source::Local);
        entry.path = "mods-local/custom.jar".into();
        assert_eq!(
            file_path(root, &entry).unwrap(),
            root.join("mods-local").join("custom.jar")
        );
        entry.path = "mods/custom.jar".into();
        assert!(file_path(root, &entry).is_err());

        entry.kind = Kind::Resourcepack;
        entry.path = "resourcepacks-local/faithful.zip".into();
        assert_eq!(
            file_path(root, &entry).unwrap(),
            root.join("resourcepacks-local").join("faithful.zip")
        );
        entry.source = None;
        entry.path = "resourcepacks/faithful.zip".into();
        assert_eq!(
            file_path(root, &entry).unwrap(),
            root.join("resourcepacks").join("faithful.zip")
        );
        entry.path = "mods/faithful.zip".into();
        assert!(file_path(root, &entry).is_err());
    }

    #[test]
    fn stock_lists_each_kind_with_its_extension() {
        let root = tempfile::tempdir().unwrap();
        for (dir, name) in [
            ("mods", "jei.jar"),
            ("mods", "pack.zip"),
            ("mods-local", "custom.JAR"),
            ("resourcepacks", "faithful.zip"),
            ("resourcepacks", "mod.jar"),
            ("resourcepacks-local", "house.zip"),
        ] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
            fs::write(root.path().join(dir).join(name), b"x").unwrap();
        }
        let found: Vec<String> = stock(root.path()).unwrap().into_keys().collect();
        assert_eq!(
            found,
            [
                "mods-local/custom.JAR",
                "mods/jei.jar",
                "resourcepacks-local/house.zip",
                "resourcepacks/faithful.zip"
            ]
        );
        let found: Vec<String> = files_in(root.path()).unwrap().into_keys().collect();
        assert_eq!(found, ["mods/jei.jar", "resourcepacks/faithful.zip"]);
    }
}
