//! Integration tests of `modpack status`. None of them touches the network.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

/// The binary without a terminal, in `dir`, with a machine config of its own.
fn modpack(dir: &Path, home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir)
        .env_remove("RUST_LOG")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("APPDATA", home.join("AppData"));
    cmd
}

#[test]
fn status_outside_a_repository_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    modpack(dir.path(), dir.path())
        .args(["status", "--json"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains("not inside a modpack repository"));
}

#[test]
fn json_status_of_a_repository_without_instance() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".modpack")).unwrap();
    fs::write(
        repo.path().join(".modpack/socle.toml"),
        format!(
            "version = \"{}\"\n\n[init]\nname = \"P\"\nminecraft = \"1.21.1\"\n\
             loader = \"neoforge\"\nloader-version = \"21.1.77\"\nauthor = \"a\"\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    fs::write(
        repo.path().join("mods.lock.toml"),
        "[pack]\nminecraft = \"1.21.1\"\nloader = \"neoforge\"\nloader-version = \"21.1.77\"\n",
    )
    .unwrap();
    fs::create_dir_all(repo.path().join("mods")).unwrap();
    fs::write(repo.path().join("mods/by-hand.jar"), b"abc").unwrap();

    let output = modpack(repo.path(), home.path())
        .args(["status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(json["instance"].is_null());
    assert_eq!(json["manifest"]["stale"], true);
    assert_eq!(json["manifest"]["new_jars"][0], "mods/by-hand.jar");

    modpack(repo.path(), home.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("manifest: stale"));
}
