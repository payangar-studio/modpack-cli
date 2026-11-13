//! The CurseForge pack: a zip holding `manifest.json` and `overrides/`.
//!
//! CurseForge publishes no specification of this format. The layout and the
//! fields below are taken from existing tools, packwiz first
//! (`curseforge/packinterop/translation.go`, `WriteManifestFromPack`), which
//! itself matches what the CurseForge app exports.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use zip::write::SimpleFileOptions;

use super::{ids, loader};
use crate::error::{Context, Result};
use crate::platform::PackBuild;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    minecraft: Minecraft,
    manifest_type: &'static str,
    manifest_version: u32,
    name: &'a str,
    version: &'a str,
    /// The pack build carries no author; the CurseForge project has one.
    author: &'static str,
    files: Vec<FileRef>,
    overrides: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Minecraft {
    version: String,
    mod_loaders: Vec<ModLoader>,
}

#[derive(Debug, Serialize)]
struct ModLoader {
    /// `<loader>-<loader version>`, such as `neoforge-21.1.77`.
    id: String,
    primary: bool,
}

#[derive(Debug, Serialize)]
struct FileRef {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    required: bool,
}

pub fn build(build: &PackBuild<'_>) -> Result<PathBuf> {
    loader(&build.pack.loader)?;
    let files = build
        .mods
        .iter()
        .map(|entry| {
            let (project_id, file_id) = ids(entry)?;
            Ok(FileRef {
                project_id,
                file_id,
                required: true,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = Manifest {
        minecraft: Minecraft {
            version: build.pack.minecraft.clone(),
            mod_loaders: vec![ModLoader {
                id: format!("{}-{}", build.pack.loader, build.pack.loader_version),
                primary: true,
            }],
        },
        manifest_type: "minecraftModpack",
        manifest_version: 1,
        name: build.name,
        version: build.version,
        author: "",
        files,
        overrides: "overrides",
    };
    // A later override replaces an earlier one at the same path.
    let overrides: BTreeMap<&str, &PathBuf> = build
        .overrides
        .iter()
        .map(|o| (o.path.as_str(), &o.source))
        .collect();

    let path = build.out_dir.join(format!("{}.zip", build.file_stem));
    let what = format!("writing {}", path.display());
    let file = fs::File::create(&path).context(what.clone())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    zip.start_file("manifest.json", options)
        .context(what.clone())?;
    let json = serde_json::to_string_pretty(&manifest).expect("the manifest always serializes");
    zip.write_all(json.as_bytes()).context(what.clone())?;
    zip.add_directory("overrides/", options)
        .context(what.clone())?;
    for (inner, source) in overrides {
        let bytes = fs::read(source).context(format!("reading {}", source.display()))?;
        zip.start_file(format!("overrides/{inner}"), options)
            .context(what.clone())?;
        zip.write_all(&bytes).context(what.clone())?;
    }
    zip.finish().context(what)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::path::Path;

    use super::*;
    use crate::error::ErrorKind;
    use crate::model::manifest::Manifest as ModsLock;
    use crate::platform::Override;

    fn fixture() -> ModsLock {
        ModsLock::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo/mods.lock.toml"),
        )
        .unwrap()
    }

    fn read(zip_path: &Path, name: &str) -> String {
        let mut zip = zip::ZipArchive::new(fs::File::open(zip_path).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn builds_the_zip_from_the_fixture() {
        let lock = fixture();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.toml");
        let mode = dir.path().join("mode.toml");
        fs::write(&base, "base").unwrap();
        fs::write(&mode, "mode").unwrap();
        let path = build(&PackBuild {
            name: "Test Pack",
            version: "1.2.3",
            pack: &lock.pack,
            mods: vec![lock.get("ferritecore").unwrap(), lock.get("jei").unwrap()],
            overrides: vec![
                Override {
                    source: base,
                    path: "config/a.toml".into(),
                },
                Override {
                    source: mode,
                    path: "config/a.toml".into(),
                },
            ],
            out_dir: dir.path(),
            file_stem: "test-pack-default-1.2.3",
        })
        .unwrap();
        assert_eq!(path, dir.path().join("test-pack-default-1.2.3.zip"));

        let manifest: serde_json::Value =
            serde_json::from_str(&read(&path, "manifest.json")).unwrap();
        assert_eq!(
            manifest,
            serde_json::json!({
                "minecraft": {
                    "version": "1.21.1",
                    "modLoaders": [{ "id": "neoforge-21.1.77", "primary": true }]
                },
                "manifestType": "minecraftModpack",
                "manifestVersion": 1,
                "name": "Test Pack",
                "version": "1.2.3",
                "author": "",
                "files": [
                    { "projectID": 429235, "fileID": 5839826, "required": true },
                    { "projectID": 238222, "fileID": 5846877, "required": true }
                ],
                "overrides": "overrides"
            })
        );
        assert_eq!(read(&path, "overrides/config/a.toml"), "mode");
    }

    #[test]
    fn a_mod_without_curseforge_ids_stops_the_build() {
        let lock = fixture();
        let dir = tempfile::tempdir().unwrap();
        let err = build(&PackBuild {
            name: "Test Pack",
            version: "1.2.3",
            pack: &lock.pack,
            mods: vec![lock.get("sodium").unwrap()],
            overrides: Vec::new(),
            out_dir: dir.path(),
            file_stem: "x",
        })
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("`sodium`"));
    }
}
