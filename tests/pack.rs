//! `modpack pack` on a copy of the fixture repository. Nothing here touches
//! the network: eligibility and packing read the manifest only.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

/// A copy of the fixture repository, with one base config file.
fn repo() -> tempfile::TempDir {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo");
    let dir = tempfile::tempdir().unwrap();
    for file in [".modpack/socle.toml", "mods.lock.toml", "modes.toml"] {
        let to = dir.path().join(file);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(fixture.join(file), to).unwrap();
    }
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(dir.path().join("config/a.toml"), "a").unwrap();
    dir
}

fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    // A key, so that CurseForge reads neither the keyring nor blames a
    // missing key for the fixture's mods that are not on CurseForge.
    cmd.current_dir(dir)
        .env_remove("RUST_LOG")
        .env("MODPACK_CURSEFORGE_API_KEY", "test-key");
    cmd
}

#[test]
fn builds_both_platforms_of_a_mode() {
    let repo = repo();
    modpack(repo.path())
        .args(["pack", "server", "--pack-version", "2.0.0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fixture-pack-server-2.0.0.mrpack"))
        .stdout(predicate::str::contains("fixture-pack-server-2.0.0.zip"));
    assert!(
        repo.path()
            .join("dist/fixture-pack-server-2.0.0.zip")
            .is_file()
    );
}

#[test]
fn json_lists_artifacts_and_blocked_platforms_then_exits_blocked() {
    let repo = repo();
    let out = repo.path().join("out");
    let run = modpack(repo.path())
        .args([
            "pack",
            "default",
            "--json",
            "--pack-version",
            "1.0.0",
            "--out",
        ])
        .arg(&out)
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "curseforge pack of mode `default`",
        ));
    let report: serde_json::Value = serde_json::from_slice(&run.get_output().stdout).unwrap();
    assert_eq!(report["version"], "1.0.0");
    let artifacts = report["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["platform"], "modrinth");
    assert_eq!(artifacts[0]["mode"], "default");
    let path = artifacts[0]["path"].as_str().unwrap();
    assert_eq!(
        Path::new(path),
        out.join("fixture-pack-default-1.0.0.mrpack")
    );
    assert!(Path::new(path).is_file());
    let blocked = &report["blocked"][0];
    assert_eq!(blocked["platform"], "curseforge");
    let keys: Vec<&str> = blocked["mods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["iris", "sodium"]);
}
