use std::collections::BTreeMap;
use std::io::Read;

use serde_json::Value;
use tempfile::TempDir;

use super::*;
use crate::error::ErrorKind;
use crate::platform::fake::FakePlatform;

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
/// base config and overrides for the `server` mode.
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
        fixtures().join(modes::FILE_NAME),
        root.join(modes::FILE_NAME),
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
        mode: None,
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
fn builds_one_artifact_per_mode_and_platform() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    let report = build(repo.path(), &Registry::builtin(), &request(&out)).unwrap();

    assert_eq!(report.version, "1.2.3");
    assert_eq!(report.blocked, Vec::new());
    let built: Vec<(&str, &str, PathBuf)> = report
        .artifacts
        .iter()
        .map(|a| (a.mode.as_str(), a.platform.as_str(), a.path.clone()))
        .collect();
    let expected: Vec<(&str, &str, PathBuf)> = ["default", "hardcore", "server"]
        .into_iter()
        .flat_map(|mode| {
            [("modrinth", "mrpack"), ("curseforge", "zip")].map(|(platform, ext)| {
                let file = format!("fixture-pack-{mode}-1.2.3.{ext}");
                (mode, platform, out.join(file))
            })
        })
        .collect();
    assert_eq!(built, expected);
    for (_, _, path) in &expected {
        assert!(path.is_file(), "{} missing", path.display());
    }
}

#[test]
fn artifacts_hold_the_mode_mods_and_merged_overrides() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    build(repo.path(), &Registry::builtin(), &request(&out)).unwrap();

    // The server mode removes sodium, iris and jei, and hardcore-tweaks is
    // removed from the default mode.
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

    // A mode without overrides gets the base config only, and its own mods.
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
    let report = build(repo.path(), &Registry::builtin(), &request(&out)).unwrap();

    let built: Vec<(&str, &str)> = report
        .artifacts
        .iter()
        .map(|a| (a.mode.as_str(), a.platform.as_str()))
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
            (b.mode.as_str(), b.platform.as_str(), keys)
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
    assert!(text.contains("curseforge pack of mode `default`:"));
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
        &Registry::builtin(),
        &Request {
            mode: Some("hardcore"),
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
        (blocked.mode.as_str(), blocked.platform.as_str()),
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
fn mode_and_platform_narrow_the_build() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    let report = build(
        repo.path(),
        &Registry::builtin(),
        &Request {
            mode: Some("server"),
            platform: Some("curseforge"),
            ..request(&out)
        },
    )
    .unwrap();
    assert_eq!(
        report.artifacts,
        [Artifact {
            mode: "server".into(),
            platform: "curseforge".into(),
            path: out.join("fixture-pack-server-1.2.3.zip"),
        }]
    );
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
}

#[test]
fn unknown_mode_or_platform_is_a_usage_error() {
    let repo = repo(everywhere);
    let out = repo.path().join("dist");
    let registry = Registry::builtin();
    let unknown_mode = Request {
        mode: Some("lowend"),
        ..request(&out)
    };
    let err = build(repo.path(), &registry, &unknown_mode).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("unknown mode `lowend`"));

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
            mode: "default".into(),
            platform: "modrinth".into(),
            path: "dist/p-default-1.0.0.mrpack".into(),
        }],
        blocked: vec![Blocked {
            mode: "default".into(),
            platform: "curseforge".into(),
            mods: vec![BlockedMod {
                key: "iris".into(),
                reason: "why".into(),
            }],
        }],
    };
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({
            "version": "1.0.0",
            "artifacts": [
                { "mode": "default", "platform": "modrinth", "path": "dist/p-default-1.0.0.mrpack" }
            ],
            "blocked": [
                { "mode": "default", "platform": "curseforge", "mods": [{ "key": "iris", "reason": "why" }] }
            ]
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
