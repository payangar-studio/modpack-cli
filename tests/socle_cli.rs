//! Integration tests of `modpack init` and `modpack update`, in temporary
//! directories.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use modpack_cli::model::socle::{LockedFile, LockedKind, SocleConfig, SocleLock};
use modpack_cli::model::{sha256_bytes, sha256_file};
use predicates::prelude::*;

const INIT_ARGS: &[&str] = &[
    "--name=Test Pack",
    "--mc-version=1.21.1",
    "--loader=neoforge",
    "--loader-version=21.1.77",
    "--author=Tester",
];

/// The binary without a terminal (assert_cmd pipes stdin and stdout).
fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir).env_remove("RUST_LOG");
    cmd
}

/// A fresh repository in `<tmp>/pack`.
fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .arg("init")
        .arg("pack")
        .args(INIT_ARGS)
        .assert()
        .success()
        .stdout(predicate::str::contains("Created modpack `Test Pack`"));
    let root = tmp.path().join("pack");
    (tmp, root)
}

/// Pretends the repository was written by an older CLI, so that `update` runs.
fn make_older(root: &Path) {
    let mut config = SocleConfig::load(root).unwrap();
    config.version = "0.0.1".into();
    config.save(root).unwrap();
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn init_names_the_directory_once() {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .arg("init")
        .args(INIT_ARGS)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Created modpack `Test Pack` in .\n",
        ));
}

#[test]
fn init_creates_a_complete_repository() {
    let (_tmp, root) = init_repo();
    for path in [
        ".modpack/socle.toml",
        ".modpack/socle.lock",
        ".modpack/README.md",
        "mods.lock.toml",
        "modes.toml",
        "AGENTS.md",
        ".gitignore",
        "CLAUDE.md",
    ] {
        assert!(root.join(path).exists(), "{path} missing");
    }
    for dir in ["mods", "config", "overrides", ".git"] {
        assert!(root.join(dir).is_dir(), "{dir}/ missing");
    }
    assert!(read(&root, "mods.lock.toml").contains("loader-version = \"21.1.77\""));
    assert!(read(&root, ".gitignore").contains("# modpack:begin\nmods/*.jar\n"));

    let lock = SocleLock::load(&root).unwrap();
    let paths: Vec<_> = lock.files.iter().map(|f| f.path.as_str()).collect();
    // Contains rather than equals: each socle skill adds its own files.
    for path in [".gitignore", ".modpack/README.md", "AGENTS.md"] {
        assert!(paths.contains(&path), "{path} not in the lock");
    }
    let readme = lock
        .files
        .iter()
        .find(|f| f.path == ".modpack/README.md")
        .unwrap();
    assert_eq!(readme.kind, LockedKind::File);
    assert_eq!(
        sha256_file(&root.join(&readme.path)).unwrap(),
        readme.sha256
    );

    // No automatic commit.
    let log = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!log.status.success());
}

#[test]
fn init_without_terminal_and_without_name_names_the_argument() {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .args(["init", "pack", "--mc-version=1.21.1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("pass --name"));
    assert!(!tmp.path().join("pack").exists());
}

#[test]
fn init_refuses_a_directory_that_holds_a_socle() {
    let (tmp, _root) = init_repo();
    modpack(tmp.path())
        .arg("init")
        .arg("pack")
        .args(INIT_ARGS)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("already holds a modpack socle"));
}

#[test]
fn init_refuses_an_unknown_loader() {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .args(["init", "pack"])
        .args(&INIT_ARGS[..2])
        .args(["--loader=paper", "--loader-version=1", "--author=a"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown loader `paper`"));
}

#[test]
fn update_on_an_up_to_date_repository_does_nothing_and_says_so() {
    let (_tmp, root) = init_repo();
    let before = read(&root, ".modpack/socle.lock");
    fs::write(root.join(".modpack/README.md"), "edited\n").unwrap();
    modpack(&root)
        .arg("update")
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to update"));
    assert_eq!(read(&root, ".modpack/socle.lock"), before);
    assert_eq!(read(&root, ".modpack/README.md"), "edited\n");
}

#[test]
fn forced_update_on_an_up_to_date_repository_restores_the_socle() {
    let (_tmp, root) = init_repo();
    let before = read(&root, ".modpack/README.md");
    fs::write(root.join(".modpack/README.md"), "edited\n").unwrap();
    fs::write(root.join(".modpack/socle.lock"), "not toml [").unwrap();
    modpack(&root)
        .args(["update", "--force"])
        .assert()
        .success();
    assert_eq!(read(&root, ".modpack/README.md"), before);
    assert!(read(&root, ".modpack/socle.lock").contains(".modpack/README.md"));
}

#[test]
fn update_stops_on_a_hand_edited_file_unless_forced() {
    let (_tmp, root) = init_repo();
    make_older(&root);
    fs::write(root.join(".modpack/README.md"), "edited\n").unwrap();

    modpack(&root)
        .arg("update")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("- .modpack/README.md"))
        .stderr(predicate::str::contains("--force"));
    assert_eq!(read(&root, ".modpack/README.md"), "edited\n");
    assert_eq!(SocleConfig::load(&root).unwrap().version, "0.0.1");

    modpack(&root)
        .args(["update", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated the socle from 0.0.1"));
    assert_ne!(read(&root, ".modpack/README.md"), "edited\n");
    assert_eq!(
        SocleConfig::load(&root).unwrap().version,
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn update_replaces_a_block_and_keeps_the_text_around_it() {
    let (_tmp, root) = init_repo();
    make_older(&root);
    let block = read(&root, ".gitignore");
    fs::write(
        root.join(".gitignore"),
        format!("target/\n\n{block}\n*.log\n"),
    )
    .unwrap();
    // A stale block: the registry records what an older socle wrote.
    let mut lock = SocleLock::load(&root).unwrap();
    let old = "mods/*.jar\n";
    let text =
        read(&root, ".gitignore").replace("mods/*.jar\n.source/\n.modpack/local/\ndist/\n", old);
    fs::write(root.join(".gitignore"), text).unwrap();
    lock.files
        .iter_mut()
        .find(|f| f.path == ".gitignore")
        .unwrap()
        .sha256 = sha256_bytes(old.as_bytes());
    lock.save(&root).unwrap();

    modpack(&root).arg("update").assert().success();
    assert_eq!(
        read(&root, ".gitignore"),
        format!("target/\n\n{block}\n*.log\n")
    );
}

#[test]
fn update_appends_a_missing_block_to_the_end_of_the_file() {
    let (_tmp, root) = init_repo();
    make_older(&root);
    // A registry from a socle without the .gitignore block.
    let mut lock = SocleLock::load(&root).unwrap();
    lock.files.retain(|f| f.path != ".gitignore");
    lock.save(&root).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();

    modpack(&root).arg("update").assert().success();
    assert_eq!(
        read(&root, ".gitignore"),
        "target/\n\n# modpack:begin\nmods/*.jar\n.source/\n.modpack/local/\ndist/\n# modpack:end\n"
    );
}

#[test]
fn update_removes_a_file_that_left_the_templates() {
    let (_tmp, root) = init_repo();
    make_older(&root);
    let content = b"#!/bin/sh\nmodpack manifest\n";
    fs::create_dir_all(root.join(".modpack/old")).unwrap();
    fs::write(root.join(".modpack/old/hook"), content).unwrap();
    let mut lock = SocleLock::load(&root).unwrap();
    lock.files.push(LockedFile {
        path: ".modpack/old/hook".into(),
        sha256: sha256_bytes(content),
        kind: LockedKind::File,
    });
    lock.save(&root).unwrap();

    modpack(&root)
        .arg("update")
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed .modpack/old/hook"));
    assert!(!root.join(".modpack/old").exists());
    let lock = SocleLock::load(&root).unwrap();
    assert!(lock.files.iter().all(|f| f.path != ".modpack/old/hook"));
}

#[test]
fn update_never_touches_the_pack_skeleton() {
    let (_tmp, root) = init_repo();
    make_older(&root);
    fs::write(root.join("modes.toml"), "# mine\n").unwrap();
    fs::remove_file(root.join("mods/.gitkeep")).unwrap();
    let manifest = read(&root, "mods.lock.toml");

    modpack(&root).arg("update").assert().success();
    assert_eq!(read(&root, "modes.toml"), "# mine\n");
    assert!(!root.join("mods/.gitkeep").exists());
    assert_eq!(read(&root, "mods.lock.toml"), manifest);
}

#[test]
fn update_refuses_a_repository_newer_than_the_cli() {
    let (_tmp, root) = init_repo();
    let mut config = SocleConfig::load(&root).unwrap();
    config.version = "999.0.0".into();
    config.save(&root).unwrap();
    modpack(&root)
        .arg("update")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("newer than this CLI"))
        .stderr(predicate::str::contains("upgrade modpack-cli"));
}

#[test]
fn update_outside_a_repository_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .arg("update")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("modpack init"));
}
