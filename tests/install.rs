//! Integration tests of `modpack install`. None of them touches the network.

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
fn install_outside_a_repository_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    modpack(dir.path(), dir.path())
        .arg("install")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not inside a modpack repository"));
}

#[test]
fn install_on_a_complete_stock_downloads_nothing() {
    let repo = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".modpack")).unwrap();
    fs::write(
        repo.path().join(".modpack/socle.toml"),
        "version = \"0.1.0\"\n\n[init]\nname = \"P\"\nminecraft = \"1.21.1\"\n\
         loader = \"neoforge\"\nloader-version = \"21.1.77\"\nauthor = \"a\"\n",
    )
    .unwrap();
    // sha1 and sha512 of "abc"; the download URL does not resolve, so any
    // attempt to fetch it would fail the command.
    fs::write(
        repo.path().join("mods.lock.toml"),
        "[pack]\nminecraft = \"1.21.1\"\nloader = \"neoforge\"\nloader-version = \"21.1.77\"\n\n\
         [[mods]]\nkey = \"abc\"\npath = \"mods/abc.jar\"\nfileSize = 3\n\
         downloads = [\"https://modpack-cli.invalid/abc.jar\"]\n\n[mods.hashes]\n\
         sha1 = \"a9993e364706816aba3e25717850c26c9cd0d89d\"\n\
         sha512 = \"ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f\"\n",
    )
    .unwrap();
    fs::create_dir_all(repo.path().join("mods")).unwrap();
    fs::write(repo.path().join("mods/abc.jar"), b"abc").unwrap();

    modpack(repo.path(), home.path())
        .arg("install")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "jars: 1 present, 0 downloaded, 0 failed",
        ));
}
