//! Integration tests of `modpack auth`. None of them reaches the system
//! keyring: the refusal without a terminal comes first.

use assert_cmd::Command;
use predicates::prelude::*;

fn modpack() -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("RUST_LOG");
    cmd
}

#[test]
fn auth_is_listed_with_its_arguments() {
    modpack()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("  auth "));
    modpack()
        .args(["auth", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("[SECRET]")
                .and(predicate::str::contains("--status"))
                .and(predicate::str::contains("--remove <SECRET>"))
                .and(predicate::str::contains("curseforge-api-key")),
        );
}

#[test]
fn auth_without_terminal_refuses_and_points_to_the_environment() {
    modpack()
        .arg("auth")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("there is none").and(
        predicate::str::contains(
            "MODPACK_MODRINTH_TOKEN, MODPACK_CURSEFORGE_UPLOAD_TOKEN, MODPACK_CURSEFORGE_API_KEY",
        ),
    ));
    modpack()
        .args(["auth", "curseforge-api-key"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "set MODPACK_CURSEFORGE_API_KEY in the environment",
        ));
}

#[test]
fn auth_rejects_an_unknown_secret() {
    modpack()
        .args(["auth", "github-token"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("modrinth-token"));
}
