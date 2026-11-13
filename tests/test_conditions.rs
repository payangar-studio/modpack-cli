//! Integration tests of `modpack test`. None of them starts a server.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir).env_remove("RUST_LOG");
    cmd
}

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
}

#[test]
fn static_conditions_pass_on_the_fixture() {
    modpack(&fixture())
        .args(["test", "server", "--condition", "side"])
        .assert()
        .success()
        .stdout("server\n  passed   side = \"server\"\n1 passed, 0 failed, 0 skipped\n");
}

#[test]
fn json_report_is_printed_on_stdout() {
    let output = modpack(&fixture())
        .args(["test", "--condition", "deps-complete", "--json"])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(report["passed"], 3);
    assert_eq!(report["variants"].as_array().unwrap().len(), 3);
}

#[test]
fn test_outside_a_repository_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    modpack(dir.path())
        .arg("test")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not inside a modpack repository"));
}

#[test]
fn a_failed_condition_exits_with_the_blocked_code() {
    let dir = tempfile::tempdir().unwrap();
    let src = fixture();
    std::fs::create_dir_all(dir.path().join(".modpack")).unwrap();
    for file in [
        ".modpack/socle.toml",
        ".modpack/socle.lock",
        "mods.lock.toml",
    ] {
        std::fs::copy(src.join(file), dir.path().join(file)).unwrap();
    }
    std::fs::write(
        dir.path().join("variants.toml"),
        "[default.conditions]\nside = \"server\"\n",
    )
    .unwrap();
    modpack(dir.path())
        .arg("test")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("0 passed, 1 failed, 0 skipped"))
        .stderr(predicate::str::contains("1 condition(s) failed"));
}
