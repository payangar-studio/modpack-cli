use super::*;
use crate::error::ErrorKind;
use crate::model::sha256_file;
use crate::model::socle::InitValues;
use crate::platform::Registry;
use crate::store::download::Fetch;
use crate::store::download::tests::{MapFetch, NoFetch, entry, manifest};

const PACK: &str = "Test Pack";

/// Says Minecraft runs, or not, whatever the instance.
struct Game(bool);

impl GameDetector for Game {
    fn is_running(&self, _instance_dir: &Path) -> Result<bool> {
        Ok(self.0)
    }
}

/// A repository whose manifest lists `a`, `b`, `c` and `extra`, with their
/// jars in `mods/`; an instance registered for the pack on the machine.
///
/// The default mode removes `extra`; `server` removes `a` and adds `extra`.
/// Base config: `config/a.toml`, `config/sub/b.toml`; `server` overrides
/// `config/a.toml` and adds `config/server.toml`.
struct Setup {
    repo: tempfile::TempDir,
    machine: tempfile::TempDir,
    registry: Registry,
}

impl Setup {
    fn new() -> Self {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        SocleConfig {
            version: "0.1.0".into(),
            init: InitValues {
                name: PACK.into(),
                minecraft: "1.21.1".into(),
                loader: "neoforge".into(),
                loader_version: "21.1.77".into(),
                author: "a".into(),
            },
        }
        .save(root)
        .unwrap();
        let keys = ["a", "b", "c", "extra"];
        manifest(keys.iter().map(|k| entry(k, k.as_bytes())).collect())
            .save(&root.join(manifest::FILE_NAME))
            .unwrap();
        for key in keys {
            write(root, &format!("mods/{key}-1.0.0.jar"), key);
        }
        write(
            root,
            modes::FILE_NAME,
            "[default]\nremove = [\"extra\"]\n\n[modes.server]\nremove = [\"a\"]\nadd = [\"extra\"]\n",
        );
        write(root, "config/a.toml", "base a");
        write(root, "config/sub/b.toml", "base b");
        write(root, "config/.gitkeep", "");
        write(root, "overrides/server/config/a.toml", "server a");
        write(root, "overrides/server/config/server.toml", "server only");

        let machine = tempfile::tempdir().unwrap();
        let setup = Self {
            repo,
            machine,
            registry: Registry::new(Vec::new()),
        };
        fs::create_dir_all(setup.instance()).unwrap();
        let mut instances = Instances::default();
        instances.set(PACK, setup.instance());
        instances.save(&setup.instances_file()).unwrap();
        setup
    }

    fn root(&self) -> &Path {
        self.repo.path()
    }

    fn instance(&self) -> PathBuf {
        self.machine.path().join("profiles").join(PACK)
    }

    fn instances_file(&self) -> PathBuf {
        self.machine.path().join("instances.toml")
    }

    fn load_with(
        &self,
        mode: &str,
        force: bool,
        running: bool,
        fetch: &dyn Fetch,
    ) -> Result<LoadReport> {
        let instances_file = self.instances_file();
        let ctx = LoadContext {
            root: self.root(),
            instances_file: &instances_file,
            game: &Game(running),
            sources: Sources {
                registry: &self.registry,
                fetch,
            },
        };
        load(&ctx, mode, force)
    }

    fn load(&self, mode: &str, force: bool) -> Result<LoadReport> {
        self.load_with(mode, force, false, &NoFetch)
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.instance().join(rel)).unwrap()
    }

    fn state(&self) -> InstanceState {
        InstanceState::load(&self.instance()).unwrap()
    }

    /// Jar files of the instance's `mods/`, sorted.
    fn jars(&self) -> Vec<String> {
        instance_jars(&self.instance())
            .unwrap()
            .into_keys()
            .collect()
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The state's files as they are on disk now: path to sha256.
fn on_disk(instance: &Path, paths: &[&str]) -> BTreeMap<String, String> {
    paths
        .iter()
        .map(|p| ((*p).to_owned(), sha256_file(&instance.join(p)).unwrap()))
        .collect()
}

#[test]
fn first_load_copies_the_mode_jars_and_writes_the_config() {
    let s = Setup::new();
    let report = s.load(modes::DEFAULT_MODE, false).unwrap();

    assert_eq!(
        report.copied,
        ["mods/a-1.0.0.jar", "mods/b-1.0.0.jar", "mods/c-1.0.0.jar"]
    );
    assert_eq!(report.written, ["config/a.toml", "config/sub/b.toml"]);
    assert!(report.removed.is_empty() && report.unknown.is_empty());
    assert!(report.downloaded.is_empty() && report.forced.is_empty());
    assert_eq!(s.jars(), report.copied);
    assert_eq!(s.read("config/a.toml"), "base a");
    assert!(!s.instance().join("config/.gitkeep").exists());

    let text = report.to_string();
    assert!(text.contains("mode `default` loaded into"), "{text}");
    assert!(text.contains("jars copied: mods/a-1.0.0.jar"), "{text}");
}

#[test]
fn state_reflects_exactly_the_files_written() {
    let s = Setup::new();
    s.load(modes::DEFAULT_MODE, false).unwrap();
    let state = s.state();
    assert_eq!(state.mode.as_deref(), Some("default"));
    assert_eq!(
        state.files,
        on_disk(
            &s.instance(),
            &[
                "config/a.toml",
                "config/sub/b.toml",
                "mods/a-1.0.0.jar",
                "mods/b-1.0.0.jar",
                "mods/c-1.0.0.jar",
            ]
        )
    );

    // Back from `server`, its own config file stays on disk but is no longer managed.
    s.load("server", false).unwrap();
    s.load(modes::DEFAULT_MODE, false).unwrap();
    assert_eq!(s.read("config/server.toml"), "server only");
    assert_eq!(s.state(), state);
}

#[test]
fn switching_mode_removes_and_adds_jars() {
    let s = Setup::new();
    s.load(modes::DEFAULT_MODE, false).unwrap();

    let report = s.load("server", false).unwrap();
    assert_eq!(report.removed, ["mods/a-1.0.0.jar"]);
    assert_eq!(report.copied, ["mods/extra-1.0.0.jar"]);
    assert_eq!(
        s.jars(),
        [
            "mods/b-1.0.0.jar",
            "mods/c-1.0.0.jar",
            "mods/extra-1.0.0.jar"
        ]
    );
    let state = s.state();
    assert_eq!(state.mode.as_deref(), Some("server"));
    assert!(!state.files.contains_key("mods/a-1.0.0.jar"));
    assert!(state.files.contains_key("mods/b-1.0.0.jar"));
    assert!(state.files.contains_key("mods/extra-1.0.0.jar"));

    let report = s.load(modes::DEFAULT_MODE, false).unwrap();
    assert_eq!(report.removed, ["mods/extra-1.0.0.jar"]);
    assert_eq!(report.copied, ["mods/a-1.0.0.jar"]);
}

#[test]
fn a_known_jar_under_another_name_counts_as_present() {
    let s = Setup::new();
    write(&s.instance(), "mods/renamed-b.jar", "b");
    let report = s.load(modes::DEFAULT_MODE, false).unwrap();
    assert_eq!(report.copied, ["mods/a-1.0.0.jar", "mods/c-1.0.0.jar"]);
    assert!(s.state().files.contains_key("mods/renamed-b.jar"));

    let report = s.load("server", false).unwrap();
    assert_eq!(report.removed, ["mods/a-1.0.0.jar"]);
    assert!(s.instance().join("mods/renamed-b.jar").exists());
}

#[test]
fn a_jar_unknown_to_the_manifest_is_never_touched() {
    let s = Setup::new();
    write(&s.instance(), "mods/tried.jar", "tried in the app");
    let report = s.load(modes::DEFAULT_MODE, false).unwrap();
    assert_eq!(report.unknown, ["mods/tried.jar"]);
    assert!(
        report
            .to_string()
            .contains("not in the manifest, left as is: mods/tried.jar")
    );

    let report = s.load("server", false).unwrap();
    assert_eq!(report.unknown, ["mods/tried.jar"]);
    assert_eq!(s.read("mods/tried.jar"), "tried in the app");
    assert!(!s.state().files.contains_key("mods/tried.jar"));
}

#[test]
fn drift_blocks_then_force_overwrites() {
    let s = Setup::new();
    s.load(modes::DEFAULT_MODE, false).unwrap();
    write(&s.instance(), "config/a.toml", "changed in game");
    fs::remove_file(s.instance().join("mods/b-1.0.0.jar")).unwrap();

    let err = s.load("server", false).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Blocked);
    let text = err.to_string();
    assert!(
        text.contains("2 file(s) of the instance changed")
            && text.contains("\n  - config/a.toml")
            && text.contains("\n  - mods/b-1.0.0.jar"),
        "{text}"
    );
    assert!(err.hint().contains("--force"));
    assert_eq!(s.read("config/a.toml"), "changed in game");
    assert_eq!(s.state().mode.as_deref(), Some("default"));

    let report = s.load("server", true).unwrap();
    assert_eq!(report.forced, ["config/a.toml", "mods/b-1.0.0.jar"]);
    assert_eq!(s.read("config/a.toml"), "server a");
    assert!(s.instance().join("mods/b-1.0.0.jar").exists());
    assert!(s.load("server", false).is_ok());
}

#[test]
fn a_foreign_file_where_a_jar_goes_blocks_without_force() {
    let s = Setup::new();
    write(&s.instance(), "mods/a-1.0.0.jar", "someone else's a");
    let err = s.load(modes::DEFAULT_MODE, false).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Blocked);
    assert!(err.to_string().contains("- mods/a-1.0.0.jar"));
    assert_eq!(s.read("mods/a-1.0.0.jar"), "someone else's a");

    let report = s.load(modes::DEFAULT_MODE, true).unwrap();
    assert_eq!(report.forced, ["mods/a-1.0.0.jar"]);
    assert!(report.unknown.is_empty());
    assert_eq!(s.read("mods/a-1.0.0.jar"), "a");
}

#[test]
fn running_minecraft_is_refused_and_nothing_is_written() {
    let s = Setup::new();
    let err = s
        .load_with(modes::DEFAULT_MODE, true, true, &NoFetch)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Blocked);
    assert!(err.to_string().contains("Minecraft is running"));
    assert!(s.jars().is_empty());
    assert!(!s.instance().join(instance::STATE_FILE).exists());
}

#[test]
fn mode_config_overrides_only_the_files_it_has() {
    let s = Setup::new();
    write(&s.instance(), "config/other.toml", "the instance's own");
    let report = s.load("server", false).unwrap();
    assert_eq!(
        report.written,
        ["config/a.toml", "config/server.toml", "config/sub/b.toml"]
    );
    assert_eq!(s.read("config/a.toml"), "server a");
    assert_eq!(s.read("config/sub/b.toml"), "base b");
    assert_eq!(s.read("config/other.toml"), "the instance's own");
    assert!(!s.state().files.contains_key("config/other.toml"));

    s.load(modes::DEFAULT_MODE, false).unwrap();
    assert_eq!(s.read("config/a.toml"), "base a");
}

#[test]
fn jar_missing_from_the_repository_is_downloaded_first() {
    let s = Setup::new();
    let mut m = Manifest::load(&s.root().join(manifest::FILE_NAME)).unwrap();
    m.get_mut("c").unwrap().downloads = vec!["https://cdn.invalid/c.jar".into()];
    m.save(&s.root().join(manifest::FILE_NAME)).unwrap();
    fs::remove_file(s.root().join("mods/c-1.0.0.jar")).unwrap();
    fs::remove_file(s.root().join("mods/extra-1.0.0.jar")).unwrap();

    let fetch = MapFetch::default().with("https://cdn.invalid/c.jar", b"c");
    let report = s
        .load_with(modes::DEFAULT_MODE, false, false, &fetch)
        .unwrap();
    assert_eq!(report.downloaded, ["c"], "extra is not in the mode");
    assert_eq!(s.read("mods/c-1.0.0.jar"), "c");
    assert!(!s.root().join("mods/extra-1.0.0.jar").exists());

    let err = s.load("server", false).unwrap_err();
    assert!(err.to_string().contains("could not be downloaded: extra"));
    assert_eq!(s.state().mode.as_deref(), Some("default"));
}

#[test]
fn no_instance_says_to_run_setup() {
    let s = Setup::new();
    Instances::default().save(&s.instances_file()).unwrap();
    let err = s.load(modes::DEFAULT_MODE, false).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(
        err.to_string()
            .contains("`Test Pack` has no Modrinth instance")
    );
    assert!(err.hint().contains("modpack setup"));
}

#[test]
fn unknown_mode_is_a_usage_error() {
    let s = Setup::new();
    let err = s.load("lowend", false).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.hint().contains("default, server"));
}

#[test]
fn unknown_mode_is_reported_before_a_missing_instance() {
    let s = Setup::new();
    Instances::default().save(&s.instances_file()).unwrap();
    let err = s.load("lowend", false).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("lowend"), "{err}");
    assert!(!err.to_string().contains("no Modrinth instance"), "{err}");
}

#[test]
fn loaded_mode_reads_the_instance_state() {
    let s = Setup::new();
    let loaded = || loaded_mode(s.root(), &s.instances_file()).unwrap();
    assert_eq!(loaded(), None);
    s.load("server", false).unwrap();
    assert_eq!(loaded().as_deref(), Some("server"));
    fs::remove_dir_all(s.instance()).unwrap();
    assert_eq!(loaded(), None);
}
