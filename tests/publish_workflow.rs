//! The publish workflow the socle lays down, and the reusable workflow it calls.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use modpack_cli::model::socle::{SocleConfig, SocleLock};

const CALLER: &str = ".github/workflows/modpack-publish.yml";

fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir).env_remove("RUST_LOG");
    cmd
}

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

fn template() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("templates/socle")
            .join(CALLER),
    )
    .unwrap()
}

#[test]
fn init_lays_down_the_publish_workflow() {
    let (_tmp, root) = init_repo();
    assert_eq!(fs::read_to_string(root.join(CALLER)).unwrap(), template());
    assert!(
        SocleLock::load(&root)
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == CALLER)
    );
}

#[test]
fn update_lays_down_the_publish_workflow_in_an_older_repository() {
    let (_tmp, root) = init_repo();
    // A repository from before the workflow existed: neither the file nor
    // its registry entry.
    fs::remove_file(root.join(CALLER)).unwrap();
    let mut lock = SocleLock::load(&root).unwrap();
    lock.files.retain(|f| f.path != CALLER);
    lock.save(&root).unwrap();
    let mut config = SocleConfig::load(&root).unwrap();
    config.version = "0.0.1".into();
    config.save(&root).unwrap();

    modpack(&root).arg("update").assert().success();

    assert_eq!(fs::read_to_string(root.join(CALLER)).unwrap(), template());
    assert!(
        SocleLock::load(&root)
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == CALLER)
    );
}

/// The caller pins the reusable workflow and the CLI to the release that
/// embeds it, so that `modpack update` moves both together.
#[test]
fn the_caller_pins_this_release() {
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let text = template();
    assert!(
        text.contains(&format!(
            "uses: payangar-studio/modpack-cli/.github/workflows/publish-modpack.yml@{tag}\n"
        )),
        "the reusable workflow is not pinned to {tag}"
    );
    assert!(
        text.contains(&format!("modpack-version: {tag}\n")),
        "the CLI is not pinned to {tag}"
    );
}
