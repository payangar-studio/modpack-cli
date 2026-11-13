//! Tests of `server-starts`. The default ones run a fake `java`, a shell
//! script that plays the installer and the server; the real one is ignored.

use super::*;
use crate::store::download::tests::{MapFetch, NoFetch, entry, manifest};

/// A machine whose Java does not exist and that fetches nothing.
pub(in crate::conditions) fn machine_without_network() -> Machine<'static> {
    Machine {
        fetch: &NoFetch,
        java: PathBuf::from("/nonexistent/modpack-test/bin/java"),
        cache_dir: PathBuf::from("/nonexistent/modpack-test/cache"),
    }
}

fn variant(keys: &[&str]) -> Resolved {
    Resolved {
        name: "server".into(),
        keys: keys.iter().map(|k| (*k).to_owned()).collect(),
        conditions: Vec::new(),
    }
}

fn pack_manifest(loader: &str, version: &str) -> Manifest {
    let mut m = manifest(vec![entry("alpha", b"alpha"), entry("beta", b"beta")]);
    m.pack.loader = loader.into();
    m.pack.loader_version = version.into();
    m
}

fn failure(status: Status) -> String {
    match status {
        Status::Failed(reason) => reason,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn other_loaders_are_skipped_with_the_reason() {
    let status = server_starts(
        Path::new("."),
        &pack_manifest("forge", "52.0.1"),
        &variant(&[]),
        Duration::from_secs(1),
        &machine_without_network(),
    );
    assert_eq!(
        status,
        Status::Skipped(
            "no scriptable server install for the loader `forge` (handled: fabric, neoforge)"
                .into()
        )
    );
}

#[test]
fn missing_java_is_skipped_and_says_how_to_point_to_one() {
    let status = server_starts(
        Path::new("."),
        &pack_manifest("fabric", "0.16.9"),
        &variant(&[]),
        Duration::from_secs(1),
        &machine_without_network(),
    );
    assert_eq!(status.name(), "skipped");
    let reason = status.reason().unwrap();
    assert!(
        reason.starts_with("no Java found") && reason.contains("set JAVA_HOME"),
        "{reason}"
    );
}

#[test]
fn names_are_made_safe_for_directories() {
    assert_eq!(
        safe_name("neoforge-1.21.1-21.1.77"),
        "neoforge-1.21.1-21.1.77"
    );
    assert_eq!(safe_name("a/../b c"), "a_.._b_c");
}

#[cfg(unix)]
mod fake {
    use super::*;
    use crate::conditions::server::java::tests::mojang;
    use crate::conditions::server::loader::{FABRIC_INSTALLERS, NEOFORGE_MAVEN};
    use std::os::unix::fs::PermissionsExt;

    const FABRIC_INSTALLER_URL: &str = "https://maven.invalid/fabric-installer-1.1.2.jar";
    const DONE: &str = r#"echo '[Server thread/INFO]: Done (0.1s)! For help, type "help"'"#;

    /// A repository with the jars of `alpha` and `beta`, a base config and a
    /// `server` override; a cache; a directory for the fake Java.
    struct Setup {
        repo: tempfile::TempDir,
        cache: tempfile::TempDir,
        bin: tempfile::TempDir,
    }

    impl Setup {
        fn new() -> Self {
            let repo = tempfile::tempdir().unwrap();
            let r = repo.path();
            fs::create_dir_all(r.join("mods")).unwrap();
            fs::write(r.join("mods/alpha-1.0.0.jar"), b"alpha").unwrap();
            fs::write(r.join("mods/beta-1.0.0.jar"), b"beta").unwrap();
            fs::create_dir_all(r.join("config")).unwrap();
            fs::write(r.join("config/a.toml"), "base").unwrap();
            fs::write(r.join("config/b.toml"), "base").unwrap();
            fs::create_dir_all(r.join("overrides/server/config")).unwrap();
            fs::write(r.join("overrides/server/config/a.toml"), "override").unwrap();
            Self {
                repo,
                cache: tempfile::tempdir().unwrap(),
                bin: tempfile::tempdir().unwrap(),
            }
        }

        fn calls(&self) -> String {
            fs::read_to_string(self.bin.path().join("calls")).unwrap_or_default()
        }

        /// Writes the fake `java`: it reports `major` to `-version`, plays
        /// both installers, and runs `server` (shell) as the server.
        fn java(&self, major: u32, server: &str) -> PathBuf {
            let calls = self.bin.path().join("calls");
            let script = format!(
                r#"#!/bin/sh
if [ "$1" = -version ]; then echo 'openjdk version "{major}.0.5" 2024-10-15' >&2; exit 0; fi
echo "$*" >> '{calls}'
if [ "$1" = -jar ] && [ "$3" = server ]; then
  [ "$(cat "$2")" = fabric-installer ] || exit 9
  [ "$6" = -mcversion ] && [ "$7" = 1.21.1 ] && [ "$9" = 0.16.9 ] && [ "${{10}}" = -downloadMinecraft ] || exit 9
  echo launcher > "$5/fabric-server-launch.jar"; mkdir -p "$5/libraries"; echo lib > "$5/libraries/lib.jar"
  exit 0
fi
if [ "$1" = -jar ] && [ "$3" = --install-server ]; then
  [ "$(cat "$2")" = neoforge-installer ] || exit 9
  mkdir -p "$4/libraries/net/neoforged/neoforge/21.1.77"
  echo args > "$4/libraries/net/neoforged/neoforge/21.1.77/unix_args.txt"
  exit 0
fi
{server}
"#,
                calls = calls.display()
            );
            let path = self.bin.path().join("java");
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn check(&self, java: PathBuf, fetch: &dyn Fetch, m: &Manifest, keys: &[&str]) -> Status {
            let machine = Machine {
                fetch,
                java,
                cache_dir: self.cache.path().to_owned(),
            };
            server_starts(
                self.repo.path(),
                m,
                &variant(keys),
                Duration::from_secs(20),
                &machine,
            )
        }
    }

    /// A server that checks what it was given, then reaches `Done`.
    fn checking_server(launch: &str) -> String {
        format!(
            r#"[ "$*" = "{launch}" ] || {{ echo "launched with $*"; exit 1; }}
grep -q '^eula=true$' eula.txt || {{ echo 'You need to agree to the EULA'; exit 1; }}
grep -q '^server-port=[0-9]' server.properties || {{ echo 'no port'; exit 1; }}
[ -f mods/alpha-1.0.0.jar ] && [ -f mods/beta-1.0.0.jar ] || {{ echo 'mods missing'; exit 1; }}
[ "$(cat config/a.toml)" = override ] && [ "$(cat config/b.toml)" = base ] || {{ echo 'config not overridden'; exit 1; }}
{DONE}
read cmd
[ "$cmd" = stop ] || exit 1"#
        )
    }

    fn fabric_fetch() -> MapFetch {
        let installers = format!(
            r#"[{{"url":"https://maven.invalid/unstable.jar","stable":false}},{{"url":"{FABRIC_INSTALLER_URL}","stable":true}}]"#
        );
        mojang(MapFetch::default(), "1.21.1", 21)
            .with(FABRIC_INSTALLERS, installers.as_bytes())
            .with(FABRIC_INSTALLER_URL, b"fabric-installer")
    }

    #[test]
    fn fabric_server_starts_then_is_reused_from_the_cache() {
        let setup = Setup::new();
        let java = setup.java(21, &checking_server("-jar fabric-server-launch.jar nogui"));
        let m = pack_manifest("fabric", "0.16.9");

        let status = setup.check(java.clone(), &fabric_fetch(), &m, &["alpha", "beta"]);
        assert_eq!(status, Status::Passed, "calls: {}", setup.calls());

        // Second check: nothing fetched, nothing installed again.
        let status = setup.check(java, &NoFetch, &m, &["alpha", "beta"]);
        assert_eq!(status, Status::Passed);
        assert_eq!(setup.calls().matches(" server -dir ").count(), 1);
        assert_eq!(
            fs::read_to_string(setup.cache.path().join("fabric-1.21.1-0.16.9").join(READY))
                .unwrap(),
            "21"
        );
        let runs: Vec<_> = fs::read_dir(setup.cache.path().join(".runs"))
            .unwrap()
            .collect();
        assert!(runs.is_empty(), "temporary server directories are removed");
    }

    #[test]
    fn neoforge_server_starts_from_its_args_file() {
        let setup = Setup::new();
        let java = setup.java(
            21,
            &checking_server("@libraries/net/neoforged/neoforge/21.1.77/unix_args.txt nogui"),
        );
        let fetch = mojang(MapFetch::default(), "1.21.1", 21).with(
            &format!("{NEOFORGE_MAVEN}/21.1.77/neoforge-21.1.77-installer.jar"),
            b"neoforge-installer",
        );
        let m = pack_manifest("neoforge", "21.1.77");
        let status = setup.check(java, &fetch, &m, &["alpha", "beta"]);
        assert_eq!(status, Status::Passed, "calls: {}", setup.calls());
    }

    #[test]
    fn too_old_java_fails_before_installing() {
        let setup = Setup::new();
        let java = setup.java(17, DONE);
        let m = pack_manifest("fabric", "0.16.9");
        let reason = failure(setup.check(java, &fabric_fetch(), &m, &["alpha"]));
        assert!(
            reason.contains("Java 17") && reason.contains("needs Java 21"),
            "{reason}"
        );
        assert_eq!(setup.calls(), "", "no installer run");
    }

    #[test]
    fn missing_jar_fails_and_points_to_install() {
        let setup = Setup::new();
        fs::remove_file(setup.repo.path().join("mods/beta-1.0.0.jar")).unwrap();
        let java = setup.java(21, DONE);
        let m = pack_manifest("fabric", "0.16.9");
        let reason = failure(setup.check(java, &fabric_fetch(), &m, &["alpha", "beta"]));
        assert_eq!(
            reason,
            "jars missing from mods/: beta; run `modpack install`"
        );
    }

    #[test]
    fn missing_local_jar_fails_and_points_to_git() {
        let setup = Setup::new();
        let java = setup.java(21, DONE);
        let mut m = pack_manifest("fabric", "0.16.9");
        let mut custom = entry("custom", b"custom");
        custom.path = "mods-local/custom.jar".into();
        custom.source = Some(crate::model::manifest::Source::Local);
        m.upsert(custom);
        let reason = failure(setup.check(java, &fabric_fetch(), &m, &["alpha", "custom"]));
        assert_eq!(
            reason,
            "local jars missing from mods-local/: mods-local/custom.jar; restore them from git with `git checkout -- mods-local/custom.jar`"
        );
    }

    #[test]
    fn client_only_mods_are_left_out_of_the_server() {
        let setup = Setup::new();
        let r = setup.repo.path();
        fs::write(r.join("mods/gamma-1.0.0.jar"), b"gamma").unwrap();
        let mut m = pack_manifest("fabric", "0.16.9");
        let mut gamma = entry("gamma", b"gamma");
        gamma.env.server = Support::Unsupported;
        m.upsert(gamma);
        let server = checking_server("-jar fabric-server-launch.jar nogui").replace(
            DONE,
            &format!(
                "[ ! -e mods/gamma-1.0.0.jar ] || {{ echo 'client-only mod'; exit 1; }}\n{DONE}"
            ),
        );
        let java = setup.java(21, &server);
        let status = setup.check(java, &fabric_fetch(), &m, &["alpha", "beta", "gamma"]);
        assert_eq!(status, Status::Passed, "calls: {}", setup.calls());
    }

    #[test]
    fn crashing_server_fails_with_its_log() {
        let setup = Setup::new();
        let java = setup.java(
            21,
            "echo 'Loading mods'; echo 'Missing or unsupported mandatory dependencies' >&2; exit 1",
        );
        let m = pack_manifest("fabric", "0.16.9");
        let reason = failure(setup.check(java, &fabric_fetch(), &m, &["alpha"]));
        assert!(
            reason.starts_with("the server stopped before `Done`"),
            "{reason}"
        );
        assert!(
            reason.contains("Missing or unsupported mandatory dependencies"),
            "{reason}"
        );
    }

    #[test]
    fn failing_installer_fails_with_its_output() {
        let setup = Setup::new();
        let java = setup.java(21, DONE);
        // The installer gets other bytes than the fake expects, and exits 9.
        let fetch = fabric_fetch().with(FABRIC_INSTALLER_URL, b"corrupt");
        let m = pack_manifest("fabric", "0.16.9");
        let reason = failure(setup.check(java, &fetch, &m, &["alpha"]));
        assert!(
            reason.starts_with("the fabric server installer failed"),
            "{reason}"
        );
        assert!(
            !setup.cache.path().join("fabric-1.21.1-0.16.9").exists(),
            "an unfinished install is not kept"
        );
    }
}

/// A real Fabric server, without mods: downloads the installer, Minecraft and
/// the libraries, then starts the server. Needs the network and a Java 21.
#[test]
#[ignore = "downloads a Minecraft server and needs Java 21"]
fn real_fabric_server_starts() {
    let repo = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let http = crate::store::download::Http::new();
    let mut machine = Machine::from_env(&http).unwrap();
    machine.cache_dir = cache.path().to_owned();
    let mut m = manifest(Vec::new());
    m.pack.loader = "fabric".into();
    m.pack.loader_version = "0.20.1".into();
    let status = server_starts(
        repo.path(),
        &m,
        &variant(&[]),
        Duration::from_secs(300),
        &machine,
    );
    assert_eq!(status, Status::Passed);
}
