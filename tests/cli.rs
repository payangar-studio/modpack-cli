//! Integration tests of the `modpack` command line.

use assert_cmd::Command;
use predicates::prelude::*;

/// Every V1 command, as typed on the command line, with the arguments it
/// needs to get past parsing.
const COMMANDS: &[&[&str]] = &[
    &[
        "init",
        "--name=p",
        "--mc-version=1.21.1",
        "--loader=neoforge",
        "--loader-version=21.1.1",
        "--author=a",
    ],
    &["setup"],
    &["update"],
    &["add", "sodium"],
    &["remove", "sodium"],
    &["manifest"],
    &["install"],
    &["mode", "list"],
    &["mode", "load", "server"],
    &["status"],
    &["test"],
    &["pack"],
    &["page", "push"],
];

/// The binary without a terminal (assert_cmd pipes stdin and stdout) and
/// outside any modpack repository.
fn modpack() -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("RUST_LOG");
    cmd
}

#[test]
fn help_lists_every_command() {
    let top = modpack().arg("--help").assert().success();
    let top = String::from_utf8(top.get_output().stdout.clone()).unwrap();
    for words in COMMANDS {
        assert!(
            top.contains(&format!("  {} ", words[0])),
            "{} missing from --help",
            words[0]
        );
    }
    for group in ["mode", "page"] {
        let sub = modpack().args([group, "--help"]).assert().success();
        let sub = String::from_utf8(sub.get_output().stdout.clone()).unwrap();
        for words in COMMANDS.iter().filter(|w| w[0] == group) {
            assert!(
                sub.contains(&format!("  {} ", words[1])),
                "{group} {} missing",
                words[1]
            );
        }
    }
}

#[test]
fn command_help_shows_its_arguments() {
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["init"],
            &[
                "[DIR]",
                "--name",
                "--mc-version",
                "--loader",
                "--loader-version",
                "--author",
            ],
        ),
        (&["setup"], &["--no-launch"]),
        (&["update"], &["--force"]),
        (&["add"], &["<MOD>", "--version", "--mode"]),
        (&["remove"], &["<KEY>"]),
        (&["manifest"], &["--check"]),
        (&["install"], &["--force"]),
        (&["mode", "load"], &["<MODE>", "--force"]),
        (&["test"], &["[MODE]", "--condition"]),
        (&["pack"], &["[MODE]", "--platform", "--out"]),
        (&["page", "push"], &["--platform"]),
    ];
    for (words, expected) in cases {
        let mut predicate = predicate::str::contains("Usage: modpack").boxed();
        for arg in *expected {
            predicate = predicate.and(predicate::str::contains(*arg)).boxed();
        }
        modpack()
            .args(*words)
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate);
    }
}

#[test]
fn every_command_is_not_implemented_yet() {
    // init and update are tested in tests/socle_cli.rs.
    const IMPLEMENTED: &[&str] = &[
        "init", "install", "mode", "page", "status", "test", "update",
    ];
    for words in COMMANDS.iter().filter(|w| !IMPLEMENTED.contains(&w[0])) {
        modpack()
            .args(*words)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("is not implemented yet"));
    }
}

#[test]
fn init_without_terminal_names_the_missing_argument() {
    modpack()
        .arg("init")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("pass --name"));
}
