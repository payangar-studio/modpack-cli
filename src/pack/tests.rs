use std::collections::BTreeMap;
use std::io::Read;

use serde_json::Value;
use tempfile::TempDir;

use super::*;
use crate::error::ErrorKind;
use crate::platform::fake::FakePlatform;

/// The built-in platforms, CurseForge with a key: packing never calls its
/// API, and with no key it blames the key for missing ids.
fn registry() -> Registry {
    Registry::new(vec![
        Box::new(crate::platform::modrinth::Modrinth::default()),
        Box::new(crate::platform::curseforge::CurseForge::with(
            "http://unused.invalid",
            || Some("key".into()),
        )),
    ])
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
}

fn table(pairs: &[(&str, toml::Value)]) -> toml::Table {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

/// A repository made of the fixture, its manifest changed by `edit`, with
/// base config and overrides for the `server` variant.
fn repo(edit: impl FnOnce(&mut Manifest)) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".modpack")).unwrap();
    fs::copy(
        fixtures().join(".modpack/socle.toml"),
        root.join(".modpack/socle.toml"),
    )
    .unwrap();
    fs::copy(
        fixtures().join(variants::FILE_NAME),
        root.join(variants::FILE_NAME),
    )
    .unwrap();
    let mut manifest = Manifest::load(&fixtures().join(manifest::FILE_NAME)).unwrap();
    edit(&mut manifest);
    manifest.save(&root.join(manifest::FILE_NAME)).unwrap();
    for (path, text) in [
        ("config/.gitkeep", ""),
        ("config/a.toml", "base a"),
        ("config/b.toml", "base b"),
        ("config/nested/c.toml", "base c"),
        ("overrides/server/config/a.toml", "server a"),
        ("overrides/server/config/s.toml", "server only"),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    dir
}

/// Gives every fixture mod what both platforms need to pack it.
fn everywhere(manifest: &mut Manifest) {
    for (key, project) in [("iris", 455508), ("sodium", 394468), ("hardcore-tweaks", 1)] {
        let entry = manifest.get_mut(key).unwrap();
        entry.platforms.insert(
            "curseforge".into(),
            table(&[("projectID", project.into()), ("fileID", 7.into())]),
        );
    }
    let tweaks = manifest.get_mut("hardcore-tweaks").unwrap();
    tweaks.downloads =
        vec!["https://github.com/example/tweaks/releases/download/1.2.0/t.jar".into()];
    tweaks.platforms.insert(
        "modrinth".into(),
        table(&[("project_id", "tweaks".into()), ("version_id", "v1".into())]),
    );
}

fn request<'a>(out: &'a Path) -> Request<'a> {
    Request {
        variant: None,
        platform: None,
        out_dir: out,
        version: "1.2.3",
    }
}

/// Every file of a zip, by name, as text.
fn unzip(path: &Path) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut files = BTreeMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).unwrap();
        if file.is_dir() {
            continue;
        }
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        files.insert(file.name().unwrap().into_owned(), text);
    }
    files
}

fn overrides(files: &BTreeMap<String, String>) -> BTreeMap<&str, &str> {
    files
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("overrides/")?, v.as_str())))
        .collect()
}

#[test]
fn builds_one_artifact_per_variant_and_platform() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    let report = build(repo.path(), &registry(), &request(&out)).unwrap();

    assert_eq!(report.version, "1.2.3");
    assert_eq!(report.blocked, Vec::new());
    let built: Vec<(&str, &str, PathBuf)> = report
        .artifacts
        .iter()
        .map(|a| (a.variant.as_str(), a.platform.as_str(), a.path.clone()))
        .collect();
    let expected: Vec<(&str, &str, PathBuf)> = ["default", "hardcore", "server"]
        .into_iter()
        .flat_map(|variant| {
            [("modrinth", "mrpack"), ("curseforge", "zip")].map(|(platform, ext)| {
                let file = format!("fixture-pack-{variant}-1.2.3.{ext}");
                (variant, platform, out.join(file))
            })
        })
        .collect();
    assert_eq!(built, expected);
    for (_, _, path) in &expected {
        assert!(path.is_file(), "{} missing", path.display());
    }
}

#[test]
fn artifacts_hold_the_variant_mods_and_merged_overrides() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    build(repo.path(), &registry(), &request(&out)).unwrap();

    // The server variant removes sodium, iris and jei, and hardcore-tweaks is
    // removed from the default variant.
    let mrpack = unzip(&out.join("fixture-pack-server-1.2.3.mrpack"));
    let index: Value = serde_json::from_str(&mrpack["modrinth.index.json"]).unwrap();
    assert_eq!(index["name"], "Fixture Pack");
    assert_eq!(index["versionId"], "1.2.3");
    let paths: Vec<&str> = index["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["mods/ferritecore-7.0.2-neoforge.jar"]);
    let server_overrides = BTreeMap::from([
        ("config/a.toml", "server a"),
        ("config/b.toml", "base b"),
        ("config/nested/c.toml", "base c"),
        ("config/s.toml", "server only"),
    ]);
    assert_eq!(overrides(&mrpack), server_overrides);

    let cf = unzip(&out.join("fixture-pack-server-1.2.3.zip"));
    let manifest: Value = serde_json::from_str(&cf["manifest.json"]).unwrap();
    assert_eq!(manifest["name"], "Fixture Pack");
    assert_eq!(manifest["version"], "1.2.3");
    assert_eq!(
        manifest["files"],
        serde_json::json!([{ "projectID": 429235, "fileID": 5839826, "required": true }])
    );
    assert_eq!(overrides(&cf), server_overrides);

    // A variant without overrides gets the base config only, and its own mods.
    let cf = unzip(&out.join("fixture-pack-hardcore-1.2.3.zip"));
    let manifest: Value = serde_json::from_str(&cf["manifest.json"]).unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 5);
    assert_eq!(
        overrides(&cf),
        BTreeMap::from([
            ("config/a.toml", "base a"),
            ("config/b.toml", "base b"),
            ("config/nested/c.toml", "base c"),
        ])
    );
    let mrpack = unzip(&out.join("fixture-pack-default-1.2.3.mrpack"));
    let index: Value = serde_json::from_str(&mrpack["modrinth.index.json"]).unwrap();
    assert_eq!(index["files"].as_array().unwrap().len(), 4);
}

#[test]
fn an_ineligible_mod_blocks_its_platform_only() {
    // As in the fixture: iris and sodium are not on CurseForge, and
    // hardcore-tweaks is hosted on neither platform.
    let repo = repo(|_| {});
    let out = repo.path().join("dist");
    let report = build(repo.path(), &registry(), &request(&out)).unwrap();

    let built: Vec<(&str, &str)> = report
        .artifacts
        .iter()
        .map(|a| (a.variant.as_str(), a.platform.as_str()))
        .collect();
    assert_eq!(
        built,
        [
            ("default", "modrinth"),
            ("server", "modrinth"),
            ("server", "curseforge")
        ]
    );
    let blocked: Vec<(&str, &str, Vec<&str>)> = report
        .blocked
        .iter()
        .map(|b| {
            let keys = b.mods.iter().map(|m| m.key.as_str()).collect();
            (b.variant.as_str(), b.platform.as_str(), keys)
        })
        .collect();
    assert_eq!(
        blocked,
        [
            ("default", "curseforge", vec!["iris", "sodium"]),
            ("hardcore", "modrinth", vec!["hardcore-tweaks"]),
            (
                "hardcore",
                "curseforge",
                vec!["hardcore-tweaks", "iris", "sodium"]
            ),
        ]
    );
    assert!(
        report.blocked[0].mods[0]
            .reason
            .contains("not hosted on CurseForge")
    );
    assert!(report.blocked[1].mods[0].reason.contains("no download URL"));
    assert!(!out.join("fixture-pack-default-1.2.3.zip").exists());

    let err = report.blocked_error().unwrap();
    assert_eq!(err.kind(), ErrorKind::Blocked);
    let text = err.to_string();
    assert!(text.contains("curseforge pack of variant `default`:"));
    assert!(text.contains("`iris`: not hosted on CurseForge"));
    assert!(err.hint().contains("--platform"));
}

#[test]
fn a_mod_hosted_elsewhere_is_packed_where_its_platform_accepts_it() {
    // On GitHub only: no table for either platform. Modrinth accepts the
    // GitHub URL, CurseForge needs its own ids.
    let repo = repo(|m| {
        everywhere(m);
        let tweaks = m.get_mut("hardcore-tweaks").unwrap();
        tweaks.platforms.remove("modrinth");
        tweaks.platforms.remove("curseforge");
    });
    let out = repo.path().join("dist");
    let report = build(
        repo.path(),
        &registry(),
        &Request {
            variant: Some("hardcore"),
            ..request(&out)
        },
    )
    .unwrap();
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(report.artifacts[0].platform, "modrinth");
    let index: Value =
        serde_json::from_str(&unzip(&report.artifacts[0].path)["modrinth.index.json"]).unwrap();
    assert!(index["files"].as_array().unwrap().iter().any(|f| {
        f["downloads"][0]
            .as_str()
            .unwrap()
            .starts_with("https://github.com/")
    }));
    assert_eq!(report.blocked.len(), 1);
    let blocked = &report.blocked[0];
    assert_eq!(
        (blocked.variant.as_str(), blocked.platform.as_str()),
        ("hardcore", "curseforge")
    );
    assert_eq!(blocked.mods.len(), 1);
    assert_eq!(blocked.mods[0].key, "hardcore-tweaks");
    assert!(
        blocked.mods[0]
            .reason
            .starts_with("not hosted on CurseForge")
    );
}

#[test]
fn variant_and_platform_narrow_the_build() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    let report = build(
        repo.path(),
        &registry(),
        &Request {
            variant: Some("server"),
            platform: Some("curseforge"),
            ..request(&out)
        },
    )
    .unwrap();
    assert_eq!(
        report.artifacts,
        [Artifact {
            variant: "server".into(),
            platform: "curseforge".into(),
            path: out.join("fixture-pack-server-1.2.3.zip"),
        }]
    );
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn unknown_variant_or_platform_is_a_usage_error() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    let registry = registry();
    let unknown_variant = Request {
        variant: Some("lowend"),
        ..request(&out)
    };
    let err = build(repo.path(), &registry, &unknown_variant).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("unknown variant `lowend`"));

    let unknown_platform = Request {
        platform: Some("nope"),
        ..request(&out)
    };
    let err = build(repo.path(), &registry, &unknown_platform).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.hint().contains("modrinth, curseforge"));
}

/// A platform that cannot build packs by code.
struct NoPack(FakePlatform);

impl Platform for NoPack {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn capability(&self, op: Operation) -> Capability {
        match op {
            Operation::BuildPack => Capability::Unsupported,
            _ => self.0.capability(op),
        }
    }
}

#[test]
fn platforms_that_cannot_build_packs_are_skipped_unless_named() {
    let registry = Registry::new(vec![
        Box::new(NoPack(FakePlatform::new("nopack"))),
        Box::new(FakePlatform::new("fake")),
    ]);
    let names: Vec<&str> = platforms(&registry, None)
        .unwrap()
        .iter()
        .map(|p| p.name())
        .collect();
    assert_eq!(names, ["fake"]);
    let err = platforms(&registry, Some("nopack")).err().unwrap();
    assert!(err.to_string().contains("does not support `build_pack`"));
}

#[test]
fn report_serializes_for_machines() {
    let report = Report {
        version: "1.0.0".into(),
        artifacts: vec![Artifact {
            variant: "default".into(),
            platform: "modrinth".into(),
            path: "dist/p-default-1.0.0.mrpack".into(),
        }],
        blocked: vec![Blocked {
            variant: "default".into(),
            platform: "curseforge".into(),
            mods: vec![BlockedMod {
                key: "iris".into(),
                reason: "why".into(),
            }],
        }],
        local: vec![LocalMod {
            key: "custom".into(),
            path: "mods-local/custom.jar".into(),
            approved: false,
        }],
        warnings: vec!["check it".into()],
    };
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({
            "version": "1.0.0",
            "artifacts": [
                { "variant": "default", "platform": "modrinth", "path": "dist/p-default-1.0.0.mrpack" }
            ],
            "blocked": [
                { "variant": "default", "platform": "curseforge", "mods": [{ "key": "iris", "reason": "why" }] }
            ],
            "local": [{ "key": "custom", "path": "mods-local/custom.jar", "approved": false }],
            "warnings": ["check it"]
        })
    );
    assert!(
        Report {
            blocked: Vec::new(),
            ..report
        }
        .blocked_error()
        .is_none()
    );
}

/// Adds the local mod `custom` to the repository: its jar in `mods-local/`
/// and its entry, approved when `approved`.
fn add_local(root: &Path, approved: bool) {
    let jar = root.join("mods-local/custom-1.0.jar");
    fs::create_dir_all(jar.parent().unwrap()).unwrap();
    fs::write(&jar, "custom jar").unwrap();
    let hashes = hash_file(&jar).unwrap();
    let path = root.join(manifest::FILE_NAME);
    let mut manifest = Manifest::load(&path).unwrap();
    manifest.upsert(Entry {
        key: "custom".into(),
        path: "mods-local/custom-1.0.jar".into(),
        file_size: hashes.size,
        downloads: Vec::new(),
        url: None,
        source: Some(crate::model::manifest::Source::Local),
        approved: approved.then(|| hashes.sha512.clone()),
        hashes: crate::model::manifest::Hashes {
            sha1: hashes.sha1,
            sha512: hashes.sha512,
        },
        env: Default::default(),
        platforms: Default::default(),
    });
    manifest.save(&path).unwrap();
}

#[test]
fn a_local_mod_is_embedded_in_every_pack_with_a_warning() {
    let repo = repo(everywhere);
    add_local(repo.path(), false);
    let out = repo.path().join("dist");
    let report = build(
        repo.path(),
        &registry(),
        &Request {
            variant: Some("server"),
            ..request(&out)
        },
    )
    .unwrap();
    assert_eq!(
        report.artifacts.len(),
        2,
        "never blocked: {:?}",
        report.blocked
    );
    for artifact in &report.artifacts {
        let files = unzip(&artifact.path);
        assert_eq!(overrides(&files)["mods/custom-1.0.jar"], "custom jar");
    }
    let mrpack = unzip(&out.join("fixture-pack-server-1.2.3.mrpack"));
    let index: Value = serde_json::from_str(&mrpack["modrinth.index.json"]).unwrap();
    assert_eq!(
        index["files"].as_array().unwrap().len(),
        1,
        "not a download"
    );
    let cf = unzip(&out.join("fixture-pack-server-1.2.3.zip"));
    let manifest: Value = serde_json::from_str(&cf["manifest.json"]).unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 1);

    assert_eq!(
        report.local,
        [LocalMod {
            key: "custom".into(),
            path: "mods-local/custom-1.0.jar".into(),
            approved: false
        }]
    );
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    assert!(report.warnings[0].contains("modpack approve custom"));
    assert!(report.warnings[1].starts_with("curseforge (custom): "));
    assert!(report.warnings[1].contains("approved list of third-party mods"));

    // Approved, it is still reported.
    add_local(repo.path(), true);
    let report = build(repo.path(), &registry(), &request(&out)).unwrap();
    assert!(report.local[0].approved);
    assert!(
        report.warnings[0].ends_with("approved"),
        "{:?}",
        report.warnings
    );
}

#[test]
fn a_local_jar_changed_since_the_manifest_is_never_packed() {
    let repo = repo(everywhere);
    add_local(repo.path(), true);
    fs::write(repo.path().join("mods-local/custom-1.0.jar"), "edited").unwrap();
    let out = repo.path().join("dist");
    let err = build(repo.path(), &registry(), &request(&out)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.hint().contains("modpack manifest"), "{}", err.hint());

    fs::remove_file(repo.path().join("mods-local/custom-1.0.jar")).unwrap();
    let err = build(repo.path(), &registry(), &request(&out)).unwrap_err();
    assert!(err.to_string().contains("is missing"), "{err}");
}
