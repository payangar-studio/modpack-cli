//! Integration tests of `modpack page push` that need no network.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

/// The binary without a terminal, outside CI.
fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir).env_remove("RUST_LOG").env_remove("CI");
    cmd
}

/// A fresh repository in `<tmp>/pack`, with the page skeleton.
fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .args([
            "init",
            "pack",
            "--name=Test Pack",
            "--mc-version=1.21.1",
            "--loader=neoforge",
            "--loader-version=21.1.77",
            "--author=Tester",
        ])
        .assert()
        .success();
    let root = tmp.path().join("pack");
    (tmp, root)
}

#[test]
fn init_lays_down_the_page_skeleton() {
    let (_tmp, root) = init_repo();
    for path in [
        "page/page.toml",
        "page/description.md",
        "page/gallery/gallery.toml",
    ] {
        assert!(root.join(path).is_file(), "{path} missing");
    }
}

#[test]
fn skeleton_page_asks_for_a_summary_before_any_call() {
    let (_tmp, root) = init_repo();
    modpack(&root)
        .args(["page", "push", "--platform", "modrinth"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("page/page.toml has no summary"));
}

#[test]
fn refused_in_ci() {
    let (_tmp, root) = init_repo();
    modpack(&root)
        .args(["page", "push"])
        .env("CI", "true")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("never done in CI"));
}

#[test]
fn unknown_platform_lists_the_known_ones() {
    let (_tmp, root) = init_repo();
    modpack(&root)
        .args(["page", "push", "--platform", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown platform `nope`"));
}
