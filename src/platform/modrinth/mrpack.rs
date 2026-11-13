//! The `.mrpack` artifact: `modrinth.index.json` (the manifest entries
//! without the fields the manifest adds) and `overrides/`.
//!
//! Format: https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;

use serde_json::{Value, json};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::error::{Context, Error, Result};
use crate::model::manifest::{Entry, Support};
use crate::platform::PackBuild;

/// Hosts a `.mrpack` may download from; any other mod must be embedded.
pub const ALLOWED_HOSTS: [&str; 4] = [
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];

/// The entry's URLs (its `downloads`, then its direct `url`) that a `.mrpack`
/// accepts.
pub fn allowed_downloads(entry: &Entry) -> Vec<&str> {
    entry
        .downloads
        .iter()
        .map(String::as_str)
        .chain(entry.url.as_deref())
        .filter(|url| host(url).is_some_and(|h| ALLOWED_HOSTS.contains(&h.as_str())))
        .collect()
}

/// Lowercase host of an `https://` URL.
fn host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Builds `<out_dir>/<file_stem>.mrpack`. Every mod must have an allowed
/// download URL.
pub fn build(build: &PackBuild<'_>) -> Result<PathBuf> {
    let index = index(build)?;
    let path = build.out_dir.join(format!("{}.mrpack", build.file_stem));
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if let Err(e) = write_zip(&tmp, build, &index) {
        let _ = fs::remove_file(&tmp);
        return Err(e).context(format!("writing {}", path.display()));
    }
    fs::rename(&tmp, &path).context(format!("writing {}", path.display()))?;
    Ok(path)
}

fn index(build: &PackBuild<'_>) -> Result<Value> {
    let blocked: Vec<&str> = build
        .mods
        .iter()
        .filter(|e| allowed_downloads(e).is_empty())
        .map(|e| e.key.as_str())
        .collect();
    if !blocked.is_empty() {
        return Err(Error::blocked(
            format!(
                "these mods cannot be downloaded by a .mrpack: {}",
                blocked.join(", ")
            ),
            format!(
                "a .mrpack only downloads from {}; embed the jar with its author's permission, or leave the mod out of the variant",
                ALLOWED_HOSTS.join(", ")
            ),
        ));
    }
    let loader = match build.pack.loader.as_str() {
        "fabric" => "fabric-loader",
        "quilt" => "quilt-loader",
        "forge" => "forge",
        "neoforge" => "neoforge",
        other => {
            return Err(Error::usage(
                format!("loader `{other}` has no .mrpack equivalent"),
                "use one of: neoforge, fabric, forge, quilt",
            ));
        }
    };
    let files: Vec<Value> = build
        .mods
        .iter()
        .map(|e| {
            json!({
                "path": e.path,
                "hashes": { "sha1": e.hashes.sha1, "sha512": e.hashes.sha512 },
                "env": { "client": side(e.env.client), "server": side(e.env.server) },
                "downloads": allowed_downloads(e),
                "fileSize": e.file_size,
            })
        })
        .collect();
    Ok(json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": build.version,
        "name": build.name,
        "files": files,
        "dependencies": {
            "minecraft": build.pack.minecraft,
            loader: build.pack.loader_version,
        },
    }))
}

fn side(support: Support) -> &'static str {
    match support {
        Support::Required => "required",
        Support::Optional => "optional",
        Support::Unsupported => "unsupported",
    }
}

fn write_zip(path: &PathBuf, build: &PackBuild<'_>, index: &Value) -> io::Result<()> {
    let mut zip = ZipWriter::new(File::create(path)?);
    let options = SimpleFileOptions::default();
    zip.start_file("modrinth.index.json", options)
        .map_err(io::Error::other)?;
    zip.write_all(&serde_json::to_vec_pretty(index)?)?;
    // A later override replaces an earlier one at the same path.
    let mut overrides = std::collections::BTreeMap::new();
    for o in &build.overrides {
        overrides.insert(o.path.as_str(), &o.source);
    }
    for (inner, source) in overrides {
        let mut file = File::open(source)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", source.display())))?;
        zip.start_file(format!("overrides/{inner}"), options)
            .map_err(io::Error::other)?;
        io::copy(&mut file, &mut zip)?;
    }
    zip.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::manifest::Manifest;
    use crate::platform::Override;
    use std::io::Read;
    use std::path::Path;
    use zip::ZipArchive;

    fn fixture() -> Manifest {
        Manifest::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo/mods.lock.toml"),
        )
        .unwrap()
    }

    #[test]
    fn only_allowed_https_hosts_are_kept() {
        let mut entry = fixture().get("hardcore-tweaks").unwrap().clone();
        assert!(allowed_downloads(&entry).is_empty());
        entry.downloads = vec![
            "https://cdn.modrinth.com/data/a/b.jar".into(),
            "http://github.com/a/b.jar".into(),
            "https://github.com.evil.org/b.jar".into(),
            "https://user@GitLab.com:443/a/b.jar".into(),
        ];
        entry.url = Some("https://raw.githubusercontent.com/a/b.jar".into());
        assert_eq!(
            allowed_downloads(&entry),
            [
                "https://cdn.modrinth.com/data/a/b.jar",
                "https://user@GitLab.com:443/a/b.jar",
                "https://raw.githubusercontent.com/a/b.jar",
            ]
        );
    }

    #[test]
    fn builds_a_valid_mrpack_from_the_fixture() {
        let manifest = fixture();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.toml");
        let variant = dir.path().join("variant.toml");
        fs::write(&base, "base").unwrap();
        fs::write(&variant, "variant").unwrap();
        let mods: Vec<&Entry> = manifest
            .mods
            .iter()
            .filter(|e| e.key != "hardcore-tweaks")
            .collect();
        let path = build(&PackBuild {
            name: "Test Pack",
            version: "1.2.3",
            pack: &manifest.pack,
            mods: mods.clone(),
            overrides: vec![
                Override {
                    source: base,
                    path: "config/a.toml".into(),
                },
                Override {
                    source: variant,
                    path: "config/a.toml".into(),
                },
            ],
            out_dir: dir.path(),
            file_stem: "test-default-1.2.3",
        })
        .unwrap();
        assert_eq!(path, dir.path().join("test-default-1.2.3.mrpack"));

        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let index: Value =
            serde_json::from_reader(zip.by_name("modrinth.index.json").unwrap()).unwrap();
        assert_eq!(index["formatVersion"], 1);
        assert_eq!(index["game"], "minecraft");
        assert_eq!(index["versionId"], "1.2.3");
        assert_eq!(index["name"], "Test Pack");
        assert_eq!(index["dependencies"]["minecraft"], "1.21.1");
        assert_eq!(index["dependencies"]["neoforge"], "21.1.77");

        let files = index["files"].as_array().unwrap();
        assert_eq!(files.len(), mods.len());
        for (file, entry) in files.iter().zip(&mods) {
            let file = file.as_object().unwrap();
            let mut keys: Vec<&str> = file.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["downloads", "env", "fileSize", "hashes", "path"]);
            assert_eq!(file["path"], entry.path.as_str());
            assert_eq!(file["hashes"]["sha1"].as_str().unwrap().len(), 40);
            assert_eq!(file["hashes"]["sha512"].as_str().unwrap().len(), 128);
            assert!(file["env"]["client"].is_string());
            assert!(file["env"]["server"].is_string());
            assert_eq!(file["downloads"].as_array().unwrap().len(), 1);
            assert_eq!(file["fileSize"], entry.file_size);
        }
        let iris = files
            .iter()
            .find(|f| f["path"].as_str().unwrap().contains("iris"))
            .unwrap();
        assert_eq!(iris["env"]["server"], "unsupported");

        let mut text = String::new();
        zip.by_name("overrides/config/a.toml")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(
            text, "variant",
            "the variant override replaces the base config"
        );
        assert_eq!(zip.len(), 2);
    }

    #[test]
    fn a_mod_without_allowed_download_blocks_the_pack() {
        let manifest = fixture();
        let dir = tempfile::tempdir().unwrap();
        let err = build(&PackBuild {
            name: "Test",
            version: "1.0.0",
            pack: &manifest.pack,
            mods: manifest.mods.iter().collect(),
            overrides: Vec::new(),
            out_dir: dir.path(),
            file_stem: "test",
        })
        .unwrap_err();
        assert!(err.to_string().contains("hardcore-tweaks"));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn fabric_loader_gets_its_mrpack_name() {
        let mut manifest = fixture();
        manifest.pack.loader = "fabric".into();
        let sodium = manifest.get("sodium").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let index = index(&PackBuild {
            name: "T",
            version: "1",
            pack: &manifest.pack,
            mods: vec![sodium],
            overrides: Vec::new(),
            out_dir: dir.path(),
            file_stem: "t",
        })
        .unwrap();
        assert_eq!(index["dependencies"]["fabric-loader"], "21.1.77");
    }
}
