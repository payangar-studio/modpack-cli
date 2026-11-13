//! Tests of the socle's git hooks in a real temporary git repository, set up
//! by `modpack init` and `modpack setup --no-launch`. The hooks then call a
//! fake `modpack` that records its calls, so that they run without network.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Records its arguments; `manifest` appends the jars of mods/ to the
/// manifest, as a regeneration would, or fails when `FAKE_FAIL` is set.
const FAKE_MODPACK: &str = "#!/bin/sh\n\
    echo \"$*\" >> \"$FAKE_LOG\"\n\
    if [ \"$1\" = manifest ]; then\n\
    \x20   [ -z \"$FAKE_FAIL\" ] || exit 1\n\
    \x20   for jar in mods/*.jar; do [ -e \"$jar\" ] && echo \"# $jar\" >> mods.lock.toml; done\n\
    fi\n\
    exit 0\n";

struct Repo {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    /// A pack repository with the hooks enabled and a first commit, made
    /// with the fake `modpack` in `PATH`.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("modpack"), FAKE_MODPACK).unwrap();
        fs::set_permissions(bin.join("modpack"), fs::Permissions::from_mode(0o755)).unwrap();
        let repo = Self {
            root: tmp.path().join("pack"),
            tmp,
        };
        for args in [
            &[
                "init",
                "pack",
                "--name=Test Pack",
                "--mc-version=1.21.1",
                "--loader=neoforge",
                "--loader-version=21.1.77",
                "--author=Tester",
            ][..],
            &["setup", "--no-launch"][..],
        ] {
            let dir = if args[0] == "init" {
                repo.tmp.path().to_owned()
            } else {
                repo.root.clone()
            };
            let out = repo
                .env(Command::new(env!("CARGO_BIN_EXE_modpack")))
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "modpack {args:?}: {out:?}");
        }
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "--quiet", "-m", "init"]);
        repo
    }

    /// Isolated from the user's git and machine configuration, with the fake
    /// `modpack` first in `PATH`.
    fn env(&self, mut cmd: Command) -> Command {
        let home = self.tmp.path().join("home");
        cmd.env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("MODPACK_MODRINTH_DIR", self.tmp.path().join("modrinth"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Tester")
            .env("GIT_AUTHOR_EMAIL", "tester@example.invalid")
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@example.invalid")
            .env("FAKE_LOG", self.log())
            .env("PATH", self.path(true))
            .env_remove("FAKE_FAIL")
            .env_remove("RUST_LOG");
        cmd
    }

    /// `PATH` without any directory holding a real `modpack`, with the fake
    /// one first when `fake`.
    fn path(&self, fake: bool) -> OsString {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let dirs = std::env::split_paths(&current).filter(|d| !d.join("modpack").exists());
        let fake_dir = fake.then(|| self.tmp.path().join("bin"));
        std::env::join_paths(fake_dir.into_iter().chain(dirs)).unwrap()
    }

    fn log(&self) -> PathBuf {
        self.tmp.path().join("modpack.log")
    }

    /// Calls of the fake `modpack` since the last call, then forgets them.
    fn take_calls(&self) -> Vec<String> {
        let calls = fs::read_to_string(self.log()).unwrap_or_default();
        let _ = fs::remove_file(self.log());
        calls.lines().map(str::to_owned).collect()
    }

    fn run_git(&self, args: &[&str], tweak: impl FnOnce(&mut Command)) -> Output {
        let mut cmd = self.env(Command::new("git"));
        cmd.args(args).current_dir(&self.root);
        tweak(&mut cmd);
        cmd.output().unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = self.run_git(args, |_| {});
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

#[test]
fn hooks_are_laid_down_executable() {
    let repo = Repo::new();
    for hook in ["pre-commit", "post-merge", "post-checkout"] {
        let mode = fs::metadata(repo.root.join(".modpack/hooks").join(hook))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{hook}");
    }
    let tracked = repo.git(&["ls-files", "-s", ".modpack/hooks"]);
    assert_eq!(tracked.matches("100755").count(), 3, "{tracked}");
}

#[test]
fn a_commit_after_a_jar_added_by_hand_updates_the_manifest() {
    let repo = Repo::new();
    repo.take_calls();
    repo.write("mods/by-hand.jar", "jar");
    repo.write("config/a.toml", "a = 1\n");
    repo.git(&["add", "config/a.toml"]);
    repo.git(&["commit", "--quiet", "-m", "config"]);

    assert_eq!(repo.take_calls(), ["manifest"]);
    let committed = repo.git(&["show", "HEAD:mods.lock.toml"]);
    assert!(committed.contains("# mods/by-hand.jar"), "{committed}");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn a_failed_manifest_stops_the_commit() {
    let repo = Repo::new();
    repo.write("config/a.toml", "a = 1\n");
    repo.git(&["add", "config/a.toml"]);
    let out = repo.run_git(&["commit", "--quiet", "-m", "config"], |cmd| {
        cmd.env("FAKE_FAIL", "1");
    });
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--no-verify"), "{stderr}");
}

#[test]
fn a_merge_and_a_branch_change_run_install() {
    let repo = Repo::new();
    repo.git(&["switch", "--quiet", "-c", "other"]);
    repo.write("config/b.toml", "b = 1\n");
    repo.git(&["add", "config/b.toml"]);
    repo.git(&["commit", "--quiet", "-m", "b"]);
    repo.take_calls();

    repo.git(&["switch", "--quiet", "-"]);
    assert_eq!(repo.take_calls(), ["install"], "branch change");

    repo.git(&["merge", "--quiet", "--no-edit", "other"]);
    assert_eq!(repo.take_calls(), ["install"], "merge");

    repo.write("config/b.toml", "b = 2\n");
    repo.git(&["checkout", "--", "config/b.toml"]);
    assert!(
        repo.take_calls().is_empty(),
        "a file checkout is left alone"
    );
}

#[test]
fn modpack_missing_from_path_never_blocks_git() {
    let repo = Repo::new();
    repo.take_calls();
    let without = repo.path(false);
    let no_modpack = |cmd: &mut Command| {
        cmd.env("PATH", &without);
    };

    repo.write("mods/by-hand.jar", "jar");
    repo.write("config/a.toml", "a = 1\n");
    repo.git(&["add", "config/a.toml"]);
    let out = repo.run_git(&["commit", "--quiet", "-m", "config"], no_modpack);
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("modpack is not in PATH"), "{stderr}");

    let out = repo.run_git(&["switch", "--quiet", "-c", "other"], no_modpack);
    assert!(out.status.success(), "{out:?}");
    let out = repo.run_git(&["switch", "--quiet", "-"], no_modpack);
    assert!(out.status.success(), "{out:?}");
    let out = repo.run_git(&["merge", "--quiet", "other"], no_modpack);
    assert!(out.status.success(), "{out:?}");
    assert!(repo.take_calls().is_empty());
}

#[test]
fn a_failed_install_leaves_the_merge_done() {
    let repo = Repo::new();
    repo.git(&["switch", "--quiet", "-c", "other"]);
    repo.write("config/b.toml", "b = 1\n");
    repo.git(&["add", "config/b.toml"]);
    repo.git(&["commit", "--quiet", "-m", "b"]);
    repo.git(&["switch", "--quiet", "-"]);

    let failing = repo.tmp.path().join("failing");
    fs::create_dir_all(&failing).unwrap();
    fs::write(
        failing.join("modpack"),
        "#!/bin/sh\necho 'error: drift' >&2\nexit 3\n",
    )
    .unwrap();
    fs::set_permissions(failing.join("modpack"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(failing).chain(std::env::split_paths(&repo.path(false))),
    )
    .unwrap();
    let out = repo.run_git(&["merge", "--quiet", "other"], |cmd| {
        cmd.env("PATH", &path);
    });
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error: drift"), "{stderr}");
    assert!(stderr.contains("left as it is"), "{stderr}");
    assert!(Path::new(&repo.root.join("config/b.toml")).is_file());
}

#[test]
fn offline_the_real_hook_commits_a_local_jar_and_its_entry() {
    let repo = Repo::new();
    // The real `modpack` first in PATH, every platform unreachable through a
    // proxy that refuses connections.
    let real = Path::new(env!("CARGO_BIN_EXE_modpack")).parent().unwrap();
    let path = std::env::join_paths(
        std::iter::once(real.to_owned()).chain(std::env::split_paths(&repo.path(false))),
    )
    .unwrap();
    let offline = |cmd: &mut Command| {
        cmd.env("PATH", &path);
        for var in [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "ALL_PROXY",
            "https_proxy",
            "http_proxy",
            "all_proxy",
        ] {
            cmd.env(var, "http://127.0.0.1:1");
        }
        cmd.env_remove("NO_PROXY").env_remove("no_proxy");
        cmd.env("MODPACK_CURSEFORGE_API_KEY", "unused");
    };

    repo.write("mods-local/custom-tweaks-0.3.jar", "local jar");
    repo.write("mods/by-hand.jar", "jar from somewhere");
    repo.write("config/a.toml", "a = 1\n");
    repo.git(&["add", "config/a.toml"]);
    let out = repo.run_git(&["commit", "--quiet", "-m", "config"], offline);
    assert!(out.status.success(), "{out:?}");
    // Git sends a hook's output to stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("warning: modrinth skipped"), "{stderr}");

    let committed = repo.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert!(
        committed.contains("mods-local/custom-tweaks-0.3.jar"),
        "{committed}"
    );
    let manifest = repo.git(&["show", "HEAD:mods.lock.toml"]);
    assert!(manifest.contains("key = \"custom-tweaks\""), "{manifest}");
    assert!(manifest.contains("source = \"local\""), "{manifest}");
    assert!(manifest.contains("key = \"by-hand\""), "{manifest}");
    assert_eq!(repo.git(&["status", "--porcelain"]), "");

    // A jar deleted by hand keeps its entry: the hook does not commit its
    // deletion, which would leave the entry without a jar in git.
    std::fs::remove_file(repo.root.join("mods-local/custom-tweaks-0.3.jar")).unwrap();
    repo.write("config/b.toml", "b = 1\n");
    repo.git(&["add", "config/b.toml"]);
    let out = repo.run_git(&["commit", "--quiet", "-m", "config b"], offline);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        repo.git(&["ls-files", "mods-local"]),
        "mods-local/custom-tweaks-0.3.jar\n"
    );
}
