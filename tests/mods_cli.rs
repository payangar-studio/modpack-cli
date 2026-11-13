//! Integration tests of `modpack manifest`, `modpack remove` and the
//! offline refusals of `modpack add`. None of them touches the network.

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

const SOCLE: &str = "version = \"0.1.0\"\n\n[init]\nname = \"P\"\nminecraft = \"1.21.1\"\n\
    loader = \"neoforge\"\nloader-version = \"21.1.77\"\nauthor = \"a\"\n";

/// sha1 and sha512 of "abc".
const MANIFEST: &str = "[pack]\nminecraft = \"1.21.1\"\nloader = \"neoforge\"\nloader-version = \"21.1.77\"\n\n\
    [[mods]]\nkey = \"abc\"\npath = \"mods/abc.jar\"\nfileSize = 3\ndownloads = []\n\n[mods.hashes]\n\
    sha1 = \"a9993e364706816aba3e25717850c26c9cd0d89d\"\n\
    sha512 = \"ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f\"\n";

const MODES: &str = "# Hand-written modes.\n[default]\nremove = [\"abc\"]\n\n[modes.extra]\n# Only here.\nadd = [\"abc\"]\n";

/// A repository whose manifest lists `abc`, with its jar in mods/.
fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir_all(repo.path().join(".modpack")).unwrap();
    fs::write(repo.path().join(".modpack/socle.toml"), SOCLE).unwrap();
    fs::write(repo.path().join("mods.lock.toml"), MANIFEST).unwrap();
    fs::write(repo.path().join("modes.toml"), MODES).unwrap();
    fs::create_dir_all(repo.path().join("mods")).unwrap();
    fs::write(repo.path().join("mods/abc.jar"), b"abc").unwrap();
    repo
}

#[test]
fn manifest_stops_on_modes_that_do_not_match_it() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    fs::write(
        repo.path().join("modes.toml"),
        "[modes.extra]\nadd = [\"ghost\"]\n",
    )
    .unwrap();
    for args in [&["manifest"][..], &["manifest", "--check"][..]] {
        modpack(repo.path(), home.path())
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "modes.toml: inconsistent with mods.lock.toml:\n  - mode `extra` names `ghost`",
            ));
    }

    fs::write(
        repo.path().join("modes.toml"),
        "[conditions]\nfast = true\n",
    )
    .unwrap();
    modpack(repo.path(), home.path())
        .arg("manifest")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown condition `fast`"));
}

#[test]
fn manifest_check_passes_when_up_to_date_and_fails_on_a_jar_added_by_hand() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    modpack(repo.path(), home.path())
        .args(["manifest", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("up to date"));

    fs::write(repo.path().join("mods/by-hand.jar"), b"x").unwrap();
    modpack(repo.path(), home.path())
        .args(["manifest", "--check"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("new jar: mods/by-hand.jar"))
        .stderr(predicate::str::contains("modpack manifest"));
    assert_eq!(
        fs::read_to_string(repo.path().join("mods.lock.toml")).unwrap(),
        MANIFEST,
        "--check writes nothing"
    );
}

#[test]
fn remove_cleans_the_manifest_mods_and_modes() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    modpack(repo.path(), home.path())
        .args(["remove", "abc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed `abc`"));
    assert!(!repo.path().join("mods/abc.jar").exists());
    let manifest = fs::read_to_string(repo.path().join("mods.lock.toml")).unwrap();
    assert!(!manifest.contains("abc"), "{manifest}");
    assert_eq!(
        fs::read_to_string(repo.path().join("modes.toml")).unwrap(),
        "# Hand-written modes.\n[default]\nremove = []\n\n[modes.extra]\n# Only here.\nadd = []\n"
    );
    modpack(repo.path(), home.path())
        .args(["manifest", "--check"])
        .assert()
        .success();
}

#[test]
fn manifest_points_a_missing_local_jar_to_git() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    fs::remove_file(repo.path().join("mods/abc.jar")).unwrap();
    fs::create_dir_all(repo.path().join("mods-local")).unwrap();
    fs::write(repo.path().join("mods-local/custom.jar"), b"custom").unwrap();
    modpack(repo.path(), home.path())
        .arg("manifest")
        .assert()
        .success();
    fs::remove_file(repo.path().join("mods-local/custom.jar")).unwrap();
    modpack(repo.path(), home.path())
        .arg("manifest")
        .assert()
        .success()
        .stdout(
            predicate::str::contains(
                "warning: local jar missing from mods-local/, entry kept (`git checkout -- mods-local/<file>` restores it, `modpack remove <key>` removes the mod): custom",
            )
            // The jar of mods/ only: `modpack install` cannot restore a local one.
            .and(predicate::str::contains("removes the mod): abc\n")),
        );
}

#[test]
fn manifest_keeps_the_entry_of_a_missing_jar() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    fs::remove_file(repo.path().join("mods/abc.jar")).unwrap();
    modpack(repo.path(), home.path())
        .arg("manifest")
        .assert()
        .success()
        .stdout(predicate::str::contains("entry kept").and(predicate::str::contains("abc")));
    assert_eq!(
        fs::read_to_string(repo.path().join("mods.lock.toml")).unwrap(),
        MANIFEST
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("modes.toml")).unwrap(),
        MODES
    );
    modpack(repo.path(), home.path())
        .args(["manifest", "--check"])
        .assert()
        .success();
}

#[test]
fn remove_of_a_mod_whose_jar_is_missing_cleans_the_manifest_and_modes() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    fs::remove_file(repo.path().join("mods/abc.jar")).unwrap();
    modpack(repo.path(), home.path())
        .args(["remove", "abc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed `abc` from the modes"));
    let manifest = fs::read_to_string(repo.path().join("mods.lock.toml")).unwrap();
    assert!(!manifest.contains("abc"), "{manifest}");
    assert!(
        !fs::read_to_string(repo.path().join("modes.toml"))
            .unwrap()
            .contains("abc")
    );
}

#[test]
fn remove_of_an_unknown_key_is_a_usage_error() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    modpack(repo.path(), home.path())
        .args(["remove", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no mod `nope`"));
}

#[test]
fn add_to_an_unknown_mode_is_refused_offline() {
    let repo = repo();
    let home = tempfile::tempdir().unwrap();
    modpack(repo.path(), home.path())
        .args(["add", "sodium", "--mode", "lowend"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown mode `lowend`"));
}
