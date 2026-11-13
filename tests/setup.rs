//! Integration tests of `modpack setup`, with a fake Modrinth App (a script
//! that creates the instance directory, as the user's click does), a fake app
//! directory and a local HTTP server for the jars.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use assert_cmd::Command;
use predicates::prelude::*;

/// sha1 and sha512 of "abc".
const SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
const SHA512: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";

/// Stands for the Modrinth App: records the file it is given, then creates
/// the instance directory named after it.
const FAKE_APP: &str = "#!/bin/sh\n\
    echo \"$1\" >> \"$FAKE_APP_LOG\"\n\
    mkdir -p \"$MODPACK_MODRINTH_DIR/profiles/$(basename \"$1\" .mrpack)\"\n";

/// A machine of its own: home, app directory, fake app; and a fresh pack
/// repository whose manifest lists `abc`, served by `server`.
struct World {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl World {
    fn new(server: &mockito::Server) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("bin/ModrinthApp");
        fs::create_dir_all(app.parent().unwrap()).unwrap();
        fs::write(&app, FAKE_APP).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
        let world = Self {
            root: tmp.path().join("pack"),
            tmp,
        };
        world
            .modpack(world.tmp.path())
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
        fs::write(
            world.root.join("mods.lock.toml"),
            format!(
                "[pack]\nminecraft = \"1.21.1\"\nloader = \"neoforge\"\nloader-version = \"21.1.77\"\n\n\
                 [[mods]]\nkey = \"abc\"\npath = \"mods/abc.jar\"\nfileSize = 3\n\
                 downloads = [\"{}/abc.jar\"]\n\n[mods.hashes]\nsha1 = \"{SHA1}\"\nsha512 = \"{SHA512}\"\n",
                server.url()
            ),
        )
        .unwrap();
        world
    }

    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }

    fn app_log(&self) -> PathBuf {
        self.tmp.path().join("app.log")
    }

    fn instance(&self) -> PathBuf {
        self.tmp.path().join("modrinth/profiles/Test Pack")
    }

    /// The binary without a terminal, in `dir`, on this world's machine.
    fn modpack(&self, dir: &Path) -> Command {
        let home = self.home();
        let mut cmd = Command::cargo_bin("modpack").unwrap();
        cmd.current_dir(dir)
            .env_remove("RUST_LOG")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("MODPACK_MODRINTH_DIR", self.tmp.path().join("modrinth"))
            .env(
                "MODPACK_MODRINTH_APP",
                self.tmp.path().join("bin/ModrinthApp"),
            )
            .env("FAKE_APP_LOG", self.app_log());
        cmd
    }

    fn setup(&self) -> Command {
        let mut cmd = self.modpack(&self.root);
        cmd.arg("setup");
        cmd
    }

    fn hooks_path(&self) -> String {
        let out = Process::new("git")
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(&self.root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn launches(&self) -> usize {
        fs::read_to_string(self.app_log())
            .map(|log| log.lines().count())
            .unwrap_or(0)
    }
}

#[test]
fn setup_registers_the_instance_once() {
    let mut server = mockito::Server::new();
    let jar = server
        .mock("GET", "/abc.jar")
        .with_body("abc")
        .expect(1)
        .create();
    let world = World::new(&server);

    world
        .setup()
        .assert()
        .success()
        .stdout(predicate::str::contains("git hooks enabled"))
        .stdout(predicate::str::contains("1 downloaded"))
        .stdout(predicate::str::contains("confirm the import"))
        .stdout(predicate::str::contains("instance registered"))
        .stdout(predicate::str::contains("mode `default` loaded into"));
    jar.assert();
    assert_eq!(world.hooks_path(), ".modpack/hooks");
    assert_eq!(world.launches(), 1);
    let log = fs::read_to_string(world.app_log()).unwrap();
    assert!(log.trim_end().ends_with("/Test Pack.mrpack"), "{log}");
    assert!(world.instance().join("mods/abc.jar").is_file());
    let instances =
        fs::read_to_string(world.home().join(".config/modpack/instances.toml")).unwrap();
    assert!(instances.contains("\"Test Pack\""), "{instances}");

    world
        .setup()
        .assert()
        .success()
        .stdout(predicate::str::contains("git hooks already enabled"))
        .stdout(predicate::str::contains("1 present, 0 downloaded"))
        .stdout(predicate::str::contains("instance already registered"))
        .stdout(predicate::str::contains("mode `default` already loaded"));
    assert_eq!(
        world.launches(),
        1,
        "the second setup does not launch the app"
    );
}

#[test]
fn no_launch_sets_the_hooks_and_downloads_the_jars_only() {
    let mut server = mockito::Server::new();
    server.mock("GET", "/abc.jar").with_body("abc").create();
    let world = World::new(&server);

    world
        .setup()
        .arg("--no-launch")
        // No app on this machine: --no-launch never looks for it.
        .env("MODPACK_MODRINTH_APP", world.tmp.path().join("missing"))
        .assert()
        .success()
        .stdout(predicate::str::contains("git hooks enabled"))
        .stdout(predicate::str::contains("1 downloaded"))
        .stdout(predicate::str::contains("without --no-launch"));
    assert_eq!(world.hooks_path(), ".modpack/hooks");
    assert!(world.root.join("mods/abc.jar").is_file());
    assert_eq!(world.launches(), 0);
    assert!(!world.instance().exists());
}

#[test]
fn setup_outside_a_git_clone_says_so() {
    let server = mockito::Server::new();
    let world = World::new(&server);
    fs::remove_dir_all(world.root.join(".git")).unwrap();
    world
        .setup()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("git clone"));
}
