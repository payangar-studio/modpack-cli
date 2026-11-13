//! The `modpack-source` skill, laid down by the socle.

use std::fs;

use assert_cmd::Command;
use modpack_cli::model::socle::SocleLock;

const SKILL: &str = ".claude/skills/modpack-source/SKILL.md";

#[test]
fn init_lays_down_the_source_skill() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("modpack")
        .unwrap()
        .current_dir(tmp.path())
        .env_remove("RUST_LOG")
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

    let text = fs::read_to_string(root.join(SKILL)).unwrap();
    assert!(text.starts_with("---\nname: modpack-source\n"));
    assert!(
        SocleLock::load(&root)
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == SKILL)
    );

    // Clones live in the project, never in the legacy central store.
    assert!(text.contains(".source/<key>/"));
    for legacy in ["registry.json", "mods/.sources", "~/Minecraft"] {
        assert!(!text.contains(legacy), "legacy reference `{legacy}`");
    }
    assert!(!text.contains('\u{2014}'), "em dash in the skill");
}
