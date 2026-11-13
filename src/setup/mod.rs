//! The machine-side steps of `modpack setup` that no other command does:
//! enabling the socle's git hooks in a clone, and registering the pack's
//! instance in the Modrinth App.
//!
//! Git never runs the hooks of a clone until `core.hooksPath` is set for that
//! clone; the hooks themselves live in `.modpack/hooks/`, laid down by the
//! socle.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::error::{Context, Error, Result};
use crate::launcher::register::{self, Launcher, ModrinthApp};
use crate::model::instance::Instances;
use crate::model::manifest::Pack;
use crate::platform::{PackBuild, Platform};

/// Value of `core.hooksPath`, relative to the root of the clone.
pub const HOOKS_PATH: &str = ".modpack/hooks";
/// How long registration waits for the user to confirm the import in the app.
pub const REGISTER_TIMEOUT: Duration = Duration::from_secs(600);
/// Version given to the registration pack; the app shows it, nothing reads it.
const REGISTER_VERSION: &str = "dev";

/// Points `core.hooksPath` of the clone at `root` to the socle's hooks.
/// Returns false when it already did.
pub fn enable_hooks(root: &Path) -> Result<bool> {
    let prefix = git(root, &["rev-parse", "--show-prefix"])?;
    if !prefix.is_empty() {
        return Err(Error::usage(
            format!(
                "the pack is in {prefix} of its git repository, not at its root; git looks for {HOOKS_PATH} from the root"
            ),
            "keep one pack per git repository, at its root",
        ));
    }
    let current = git(
        root,
        &[
            "config",
            "--local",
            "--default",
            "",
            "--get",
            "core.hooksPath",
        ],
    )?;
    if current == HOOKS_PATH {
        return Ok(false);
    }
    git(root, &["config", "--local", "core.hooksPath", HOOKS_PATH])?;
    Ok(true)
}

/// Runs git in `dir` and returns its trimmed standard output.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let command = format!("git {}", args.join(" "));
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| {
            Error::internal(
                format!("cannot run `{command}`"),
                "install git, then run `modpack setup` again",
            )
            .with_source(e)
        })?;
    if !output.status.success() {
        return Err(Error::usage(
            format!(
                "`{command}` failed in {}: {}",
                dir.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            "run `modpack setup` in a git clone of the pack",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// What registering an instance needs from the machine.
pub struct Machine<'a> {
    /// The machine's `instances.toml`.
    pub instances_file: &'a Path,
    /// The app's `profiles/` directory.
    pub profiles: &'a Path,
    /// Where the registration `.mrpack` is written. It stays there: the app
    /// may still read it after the instance directory appears.
    pub mrpack_dir: &'a Path,
    pub launcher: &'a dyn Launcher,
    pub timeout: Duration,
}

/// Registers the pack's instance: writes the registration `.mrpack` with
/// `platform`, opens it in the app, waits for the instance directory the
/// user's click creates, and records it in the machine's `instances.toml`.
///
/// The pack holds no mod and no config, only Minecraft and the loader: the
/// default variant loaded right after fills the instance, so the app's import
/// never races the CLI over `mods/`, and a mod a `.mrpack` cannot download
/// never blocks the registration.
pub fn register_instance(
    name: &str,
    pack: &Pack,
    platform: &dyn Platform,
    machine: &Machine<'_>,
    out: &mut dyn Write,
) -> Result<PathBuf> {
    std::fs::create_dir_all(machine.mrpack_dir)
        .context(format!("creating {}", machine.mrpack_dir.display()))?;
    let mrpack = register::write_pack(
        platform,
        &PackBuild {
            name,
            version: REGISTER_VERSION,
            pack,
            mods: Vec::new(),
            overrides: Vec::new(),
            out_dir: machine.mrpack_dir,
            file_stem: name,
        },
    )?;
    writeln!(
        out,
        "Opening {} in the Modrinth App: confirm the import there. Waiting up to {} min for the instance...",
        mrpack.display(),
        machine.timeout.as_secs().div_ceil(60)
    )?;
    out.flush()?;
    let instance =
        register::register(machine.launcher, machine.profiles, &mrpack, machine.timeout)?;

    let mut instances = Instances::load(machine.instances_file)?;
    instances.set(name, instance.clone());
    instances.save(machine.instances_file)?;
    writeln!(out, "instance registered: {}", instance.display())?;
    Ok(instance)
}

/// The installed Modrinth App, looked up only when a pack is opened, so that
/// a setup that registers nothing works without the app.
pub struct InstalledApp;

impl Launcher for InstalledApp {
    fn open(&self, mrpack: &Path) -> Result<()> {
        ModrinthApp::find()?.open(mrpack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::platform::fake::FakePlatform;
    use std::fs;
    use std::sync::Mutex;

    /// Runs git in `dir` without the user's configuration.
    fn git_in(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn hooks_are_enabled_once() {
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "--quiet"]);
        assert!(enable_hooks(dir.path()).unwrap());
        assert_eq!(
            git(dir.path(), &["config", "--get", "core.hooksPath"]).unwrap(),
            HOOKS_PATH
        );
        assert!(!enable_hooks(dir.path()).unwrap());
    }

    #[test]
    fn hooks_need_the_pack_at_the_root_of_a_clone() {
        let dir = tempfile::tempdir().unwrap();
        let err = enable_hooks(dir.path()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.hint().contains("git clone"), "{err}");

        git_in(dir.path(), &["init", "--quiet"]);
        let nested = dir.path().join("pack");
        fs::create_dir_all(&nested).unwrap();
        let err = enable_hooks(&nested).unwrap_err();
        assert!(err.to_string().contains("not at its root"), "{err}");
    }

    /// Stands for the app and the user's click: creates the instance
    /// directory named after the file as soon as it is opened.
    struct ClickingApp {
        profiles: PathBuf,
        opened: Mutex<Vec<PathBuf>>,
    }

    impl Launcher for ClickingApp {
        fn open(&self, mrpack: &Path) -> Result<()> {
            self.opened.lock().unwrap().push(mrpack.to_owned());
            let name = mrpack.file_stem().unwrap();
            fs::create_dir_all(self.profiles.join(name)).unwrap();
            Ok(())
        }
    }

    #[test]
    fn registration_opens_an_empty_pack_and_records_the_instance() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        let instances_file = dir.path().join("config/instances.toml");
        let app = ClickingApp {
            profiles: profiles.clone(),
            opened: Mutex::new(Vec::new()),
        };
        let machine = Machine {
            instances_file: &instances_file,
            profiles: &profiles,
            mrpack_dir: &dir.path().join("cache"),
            launcher: &app,
            timeout: Duration::from_secs(10),
        };
        let pack = Pack {
            minecraft: "1.21.1".into(),
            loader: "neoforge".into(),
            loader_version: "21.1.77".into(),
        };
        let mut out = Vec::new();
        let instance = register_instance(
            "My: Pack",
            &pack,
            &FakePlatform::new("mrpack"),
            &machine,
            &mut out,
        )
        .unwrap();

        assert_eq!(instance, profiles.join("My_ Pack"));
        let opened = app.opened.lock().unwrap().clone();
        assert_eq!(opened, [dir.path().join("cache/My_ Pack.mrpack")]);
        let built = fs::read_to_string(&opened[0]).unwrap();
        assert!(built.contains("name My: Pack"), "{built}");
        assert!(
            !built.contains("mod ") && !built.contains("override "),
            "{built}"
        );
        assert_eq!(
            Instances::existing(&instances_file, "My: Pack").unwrap(),
            Some(instance)
        );
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("confirm the import"), "{out}");
        assert!(out.contains("up to 1 min"), "{out}");
    }
}
