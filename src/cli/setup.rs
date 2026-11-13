//! `modpack setup`: prepares a clone on this machine. Each step only does
//! what is missing, so running it again is safe:
//! 1. points `core.hooksPath` at the socle's hooks;
//! 2. downloads the missing jars (`modpack install`);
//! 3. registers the pack's instance in the Modrinth App, unless it is there
//!    already or `--no-launch` is given;
//! 4. loads the default variant into the instance, unless a variant is loaded there
//!    already (step 2 reloaded it).

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::launcher::running::{GameDetector, Processes};
use crate::launcher::{PACK_PLATFORM, dirs};
use crate::model::instance::{InstanceState, Instances};
use crate::model::manifest::{self, Manifest};
use crate::model::socle::SocleConfig;
use crate::model::variants::DEFAULT_VARIANT;
use crate::platform::Registry;
use crate::setup::{self as steps, InstalledApp, Machine, REGISTER_TIMEOUT};
use crate::store::download::{Http, Sources};
use crate::variant::{self, LoadContext};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Do not launch the Modrinth app
    #[arg(long)]
    pub no_launch: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let registry = Registry::builtin();
    let http = Http::new();
    let instances_file = Instances::default_path()?;
    let profiles = dirs::profiles_dir(None)?;
    let mrpack_dir = directories::BaseDirs::new()
        .ok_or_else(|| {
            Error::internal(
                "cannot find the user's home directory",
                "set HOME to the user's home directory",
            )
        })?
        .cache_dir()
        .join("modpack")
        .join("register");
    let machine = Machine {
        instances_file: &instances_file,
        profiles: &profiles,
        mrpack_dir: &mrpack_dir,
        launcher: &InstalledApp,
        timeout: REGISTER_TIMEOUT,
    };
    let sources = Sources {
        registry: &registry,
        fetch: &http,
    };
    setup(
        root,
        sources,
        &machine,
        &Processes,
        args.no_launch,
        &mut std::io::stdout().lock(),
    )
}

/// The command, with the machine given.
fn setup(
    root: &Path,
    sources: Sources<'_>,
    machine: &Machine<'_>,
    game: &dyn GameDetector,
    no_launch: bool,
    out: &mut dyn Write,
) -> Result<()> {
    if steps::enable_hooks(root)? {
        writeln!(
            out,
            "git hooks enabled: core.hooksPath = {}",
            steps::HOOKS_PATH
        )?;
    } else {
        writeln!(out, "git hooks already enabled")?;
    }

    super::install::install(root, sources, false, machine.instances_file, game, out)?;

    let name = SocleConfig::load(root)?.init.name;
    let Some(instance) = instance(root, &name, sources.registry, machine, no_launch, out)? else {
        return Ok(());
    };

    if let Some(variant) = InstanceState::load(&instance)?.variant {
        writeln!(out, "variant `{variant}` already loaded")?;
        return Ok(());
    }
    let ctx = LoadContext {
        root,
        instances_file: machine.instances_file,
        game,
        sources,
    };
    let report = variant::load(&ctx, DEFAULT_VARIANT, false)?;
    write!(out, "{report}")?;
    Ok(())
}

/// The pack's instance: the one already registered, or a new one, or none
/// with `--no-launch`.
fn instance(
    root: &Path,
    name: &str,
    registry: &Registry,
    machine: &Machine<'_>,
    no_launch: bool,
    out: &mut dyn Write,
) -> Result<Option<PathBuf>> {
    if let Some(dir) = Instances::existing(machine.instances_file, name)? {
        writeln!(out, "instance already registered: {}", dir.display())?;
        return Ok(Some(dir));
    }
    if no_launch {
        writeln!(
            out,
            "no Modrinth instance for this pack yet; run `modpack setup` without --no-launch to create it"
        )?;
        return Ok(None);
    }
    let platform = registry.find(PACK_PLATFORM).ok_or_else(|| {
        Error::internal(
            format!("platform `{PACK_PLATFORM}` is not built into this CLI"),
            "report this bug to the modpack-cli maintainers",
        )
    })?;
    let pack = Manifest::load(&root.join(manifest::FILE_NAME))?.pack;
    steps::register_instance(name, &pack, platform, machine, out).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::register::Launcher;
    use crate::model::socle::InitValues;
    use crate::platform::fake::FakePlatform;
    use crate::store::download::tests::{MapFetch, NoFetch, entry, manifest};
    use std::fs;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Minecraft never runs.
    struct NoGame;

    impl GameDetector for NoGame {
        fn is_running(&self, _instance_dir: &Path) -> Result<bool> {
            Ok(false)
        }
    }

    /// The app and the user's click: creates the instance directory as soon
    /// as a pack is opened, and counts the openings.
    struct ClickingApp {
        profiles: PathBuf,
        opened: Mutex<usize>,
    }

    impl Launcher for ClickingApp {
        fn open(&self, mrpack: &Path) -> Result<()> {
            *self.opened.lock().unwrap() += 1;
            fs::create_dir_all(self.profiles.join(mrpack.file_stem().unwrap())).unwrap();
            Ok(())
        }
    }

    /// A git clone of a pack whose manifest lists `jei`, served by its URL,
    /// and a machine with the app.
    struct World {
        root: tempfile::TempDir,
        machine: tempfile::TempDir,
        fetch: MapFetch,
        registry: Registry,
        app: ClickingApp,
    }

    impl World {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let status = std::process::Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(root.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()
                .unwrap();
            assert!(status.success());
            SocleConfig {
                version: "0.1.0".into(),
                init: InitValues {
                    name: "Test Pack".into(),
                    minecraft: "1.21.1".into(),
                    loader: "neoforge".into(),
                    loader_version: "21.1.77".into(),
                    author: "a".into(),
                },
            }
            .save(root.path())
            .unwrap();
            let mut jei = entry("jei", b"jei");
            jei.downloads = vec!["https://cdn.invalid/jei.jar".into()];
            manifest(vec![jei])
                .save(&root.path().join(manifest::FILE_NAME))
                .unwrap();
            fs::write(root.path().join("variants.toml"), "[default]\n").unwrap();
            let machine = tempfile::tempdir().unwrap();
            let app = ClickingApp {
                profiles: machine.path().join("profiles"),
                opened: Mutex::new(0),
            };
            Self {
                root,
                machine,
                fetch: MapFetch::default().with("https://cdn.invalid/jei.jar", b"jei"),
                registry: Registry::new(vec![Box::new(FakePlatform::new("modrinth"))]),
                app,
            }
        }

        fn instances_file(&self) -> PathBuf {
            self.machine.path().join("instances.toml")
        }

        fn setup_with(
            &self,
            fetch: &dyn crate::store::download::Fetch,
            no_launch: bool,
        ) -> (Result<()>, String) {
            let instances_file = self.instances_file();
            let profiles = self.machine.path().join("profiles");
            let mrpack_dir = self.machine.path().join("cache");
            let machine = Machine {
                instances_file: &instances_file,
                profiles: &profiles,
                mrpack_dir: &mrpack_dir,
                launcher: &self.app,
                timeout: Duration::from_secs(10),
            };
            let sources = Sources {
                registry: &self.registry,
                fetch,
            };
            let mut out = Vec::new();
            let result = setup(
                self.root.path(),
                sources,
                &machine,
                &NoGame,
                no_launch,
                &mut out,
            );
            (result, String::from_utf8(out).unwrap())
        }

        fn opened(&self) -> usize {
            *self.app.opened.lock().unwrap()
        }
    }

    #[test]
    fn full_setup_then_a_second_one_that_only_does_what_is_missing() {
        let world = World::new();
        let (result, out) = world.setup_with(&world.fetch, false);
        result.unwrap();
        assert!(out.contains("git hooks enabled"), "{out}");
        assert!(out.contains("1 downloaded"), "{out}");
        assert!(out.contains("instance registered"), "{out}");
        assert!(out.contains("variant `default` loaded into"), "{out}");
        assert_eq!(world.opened(), 1);
        let instance = world.machine.path().join("profiles/Test Pack");
        assert!(instance.join("mods/jei-1.0.0.jar").is_file());

        let (result, out) = world.setup_with(&NoFetch, false);
        result.unwrap();
        assert_eq!(world.opened(), 1, "the app is not opened again");
        assert!(out.contains("git hooks already enabled"), "{out}");
        assert!(out.contains("1 present, 0 downloaded"), "{out}");
        assert!(out.contains("instance already registered"), "{out}");
    }

    #[test]
    fn a_loaded_variant_is_kept() {
        let world = World::new();
        world.setup_with(&world.fetch, false).0.unwrap();
        fs::write(
            world.root.path().join("variants.toml"),
            "[default]\n\n[variants.server]\nremove = [\"jei\"]\n",
        )
        .unwrap();
        let instance = world.machine.path().join("profiles/Test Pack");
        let mut state = InstanceState::load(&instance).unwrap();
        state.variant = Some("server".into());
        state.save(&instance).unwrap();

        let (result, out) = world.setup_with(&NoFetch, false);
        result.unwrap();
        assert!(out.contains("variant `server` loaded into"), "{out}");
        assert!(out.contains("variant `server` already loaded"), "{out}");
        assert_eq!(
            InstanceState::load(&instance).unwrap().variant.as_deref(),
            Some("server")
        );
    }

    #[test]
    fn no_launch_sets_the_hooks_and_downloads_without_the_app() {
        let world = World::new();
        let (result, out) = world.setup_with(&world.fetch, true);
        result.unwrap();
        assert_eq!(world.opened(), 0);
        assert!(out.contains("git hooks enabled"), "{out}");
        assert!(out.contains("1 downloaded"), "{out}");
        assert!(out.contains("without --no-launch"), "{out}");
        assert!(world.root.path().join("mods/jei-1.0.0.jar").is_file());
        assert!(
            Instances::load(&world.instances_file())
                .unwrap()
                .instances
                .is_empty()
        );
    }

    #[test]
    fn an_instance_deleted_in_the_app_is_registered_again() {
        let world = World::new();
        world.setup_with(&world.fetch, false).0.unwrap();
        fs::remove_dir_all(world.machine.path().join("profiles/Test Pack")).unwrap();
        let (result, out) = world.setup_with(&NoFetch, false);
        result.unwrap();
        assert_eq!(world.opened(), 2);
        assert!(out.contains("variant `default` loaded into"), "{out}");
    }
}
