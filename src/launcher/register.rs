//! Registering a pack's instance in the Modrinth App.
//!
//! Read in the app's code: the app never discovers a directory put in
//! `profiles/`; an instance exists only in its database. So the CLI writes a
//! `.mrpack` named after the pack and opens it with the app, which works
//! whether the app runs already or not. The app asks the user to confirm the
//! import of a pack not published on Modrinth; the click creates the instance
//! directory, named after the file, which the CLI waits for.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Context, Error, Result};
use crate::platform::{PackBuild, Platform};

/// Overrides the path of the app's executable.
pub const APP_ENV: &str = "MODPACK_MODRINTH_APP";
/// Names of the app's executable: `ModrinthApp` on Linux, `Modrinth App` on
/// the other systems (`tauri.linux.conf.json` and `tauri.conf.json`).
const EXECUTABLES: [&str; 2] = ["ModrinthApp", "Modrinth App"];
/// How often the wait looks at `profiles/`.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Name of the instance directory the app creates for a pack, before any
/// ` (1)` suffix: the app's own sanitizing of a name (`sanitize_instance_name`
/// in `app-lib`). Also a valid file name for the `.mrpack`.
pub fn instance_name(pack: &str) -> String {
    pack.replace(
        ['/', '\\', '?', '*', ':', '\'', '"', '|', '<', '>', '!'],
        "_",
    )
    .trim_end_matches(['.', ' '])
    .to_owned()
}

/// Builds the registration pack with `platform` (the one named
/// [`super::PACK_PLATFORM`]), as `<out_dir>/<instance name>.mrpack`: the app
/// names the instance after the file. `build` is the default variant's; its
/// `file_stem` is replaced.
pub fn write_pack(platform: &dyn Platform, build: &PackBuild<'_>) -> Result<PathBuf> {
    let stem = instance_name(build.name);
    if stem.is_empty() {
        return Err(Error::usage(
            format!(
                "the pack name `{}` gives no usable instance name",
                build.name
            ),
            "give the pack a name with letters or digits in .modpack/socle.toml",
        ));
    }
    platform.build_pack(&PackBuild {
        file_stem: &stem,
        ..build.clone()
    })
}

/// Something that opens a `.mrpack` in the Modrinth App.
pub trait Launcher {
    fn open(&self, mrpack: &Path) -> Result<()>;
}

/// The installed Modrinth App.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModrinthApp {
    pub exe: PathBuf,
}

impl ModrinthApp {
    /// Finds the app: [`APP_ENV`], then `PATH`, then known install locations.
    pub fn find() -> Result<Self> {
        find_exe(
            std::env::var_os(APP_ENV).filter(|v| !v.is_empty()),
            std::env::var_os("PATH"),
            &known_locations(),
        )
        .map(|exe| Self { exe })
    }
}

impl Launcher for ModrinthApp {
    /// Starts the app with the file as argument and returns at once. When the
    /// app already runs, the new process hands the file to it and exits.
    fn open(&self, mrpack: &Path) -> Result<()> {
        // The running app may have another working directory.
        let mrpack =
            std::path::absolute(mrpack).context(format!("resolving {}", mrpack.display()))?;
        let mut cmd = Command::new(&self.exe);
        cmd.arg(&mrpack)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Out of the CLI's process group, so that Ctrl+C on the CLI spares the app.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        cmd.spawn().map_err(|e| {
            Error::internal(
                format!("cannot start the Modrinth App ({})", self.exe.display()),
                format!("check the app starts, or set {APP_ENV} to its executable"),
            )
            .with_source(e)
        })?;
        Ok(())
    }
}

/// Install locations to try after `PATH`: system packages, then the user's
/// own executables.
fn known_locations() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "linux") {
        dirs.extend(["/usr/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    if let Some(user) =
        directories::BaseDirs::new().and_then(|d| d.executable_dir().map(Into::into))
    {
        dirs.push(user);
    }
    dirs
}

fn find_exe(
    override_exe: Option<OsString>,
    path: Option<OsString>,
    known: &[PathBuf],
) -> Result<PathBuf> {
    if let Some(exe) = override_exe.map(PathBuf::from) {
        return if exe.is_file() {
            Ok(exe)
        } else {
            Err(Error::usage(
                format!("{APP_ENV} points to {}, which is not a file", exe.display()),
                format!("set {APP_ENV} to the Modrinth App executable, or unset it"),
            ))
        };
    }
    let dirs = path
        .iter()
        .flat_map(std::env::split_paths)
        .chain(known.iter().cloned());
    for dir in dirs {
        for name in EXECUTABLES {
            let exe = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if exe.is_file() {
                return Ok(exe);
            }
        }
    }
    Err(Error::usage(
        "cannot find the Modrinth App",
        format!("install it, or set {APP_ENV} to its executable"),
    ))
}

/// Opens `mrpack` with `launcher` and waits up to `timeout` for the instance
/// directory the import creates in `profiles`. Returns its path.
pub fn register(
    launcher: &dyn Launcher,
    profiles: &Path,
    mrpack: &Path,
    timeout: Duration,
) -> Result<PathBuf> {
    let name = mrpack
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let before = instance_dirs(profiles, &name)?;
    launcher.open(mrpack)?;
    wait_for_instance(profiles, &name, &before, timeout)
}

/// Directories of `profiles` the app may have created for `name`: `name`
/// itself, or `name (N)` when that name was taken.
pub fn instance_dirs(profiles: &Path, name: &str) -> Result<BTreeSet<String>> {
    let entries = match std::fs::read_dir(profiles) {
        Ok(entries) => entries,
        // The app creates `profiles/` with its first instance.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e).context(format!("reading {}", profiles.display())),
    };
    let mut found = BTreeSet::new();
    for entry in entries {
        let entry = entry.context(format!("reading {}", profiles.display()))?;
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        if is_instance_of(&dir_name, name) && entry.path().is_dir() {
            found.insert(dir_name);
        }
    }
    Ok(found)
}

fn is_instance_of(dir_name: &str, name: &str) -> bool {
    match dir_name.strip_prefix(name) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix(" (")
            .and_then(|r| r.strip_suffix(')'))
            .is_some_and(|n| n.parse::<u32>().is_ok()),
        None => false,
    }
}

/// Waits until a directory for `name` that is not in `before` appears in
/// `profiles`, looking every [`POLL_INTERVAL`], for at most `timeout`.
pub fn wait_for_instance(
    profiles: &Path,
    name: &str,
    before: &BTreeSet<String>,
    timeout: Duration,
) -> Result<PathBuf> {
    let deadline = Instant::now() + timeout;
    loop {
        let created = instance_dirs(profiles, name)?
            .into_iter()
            .find(|d| !before.contains(d));
        if let Some(dir) = created {
            return Ok(profiles.join(dir));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(Error::internal(
                format!(
                    "the Modrinth App created no instance for `{name}` in {} within {}s",
                    profiles.display(),
                    timeout.as_secs()
                ),
                "confirm the import in the Modrinth App, then run the command again",
            ));
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::model::manifest::Pack;
    use crate::platform::fake::FakePlatform;
    use std::fs;
    use std::sync::Mutex;

    #[test]
    fn instance_name_follows_the_app() {
        assert_eq!(instance_name("My Pack"), "My Pack");
        assert_eq!(instance_name("A/B: C?"), "A_B_ C_");
        assert_eq!(instance_name("Pack. "), "Pack");
    }

    #[test]
    fn pack_file_is_named_after_the_pack() {
        let out = tempfile::tempdir().unwrap();
        let pack = Pack {
            minecraft: "1.21.1".into(),
            loader: "fabric".into(),
            loader_version: "0.16.0".into(),
        };
        let build = PackBuild {
            name: "My: Pack",
            version: "1.0.0",
            pack: &pack,
            mods: Vec::new(),
            overrides: Vec::new(),
            out_dir: out.path(),
            file_stem: "ignored",
        };
        let path = write_pack(&FakePlatform::new("mrpack"), &build).unwrap();
        assert_eq!(path, out.path().join("My_ Pack.mrpack"));
        assert!(fs::read_to_string(path).unwrap().contains("name My: Pack"));

        let err = write_pack(
            &FakePlatform::new("mrpack"),
            &PackBuild {
                name: "..",
                ..build
            },
        )
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
    }

    #[test]
    fn finds_the_app_by_override_then_path_then_known_locations() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, known) = (dir.path().join("bin"), dir.path().join("opt"));
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&known).unwrap();
        let exe = |d: &Path| d.join(format!("ModrinthApp{}", std::env::consts::EXE_SUFFIX));
        fs::write(exe(&known), b"").unwrap();
        let path = Some(bin.clone().into_os_string());
        let known_dirs = [known.clone()];

        assert_eq!(
            find_exe(None, path.clone(), &known_dirs).unwrap(),
            exe(&known)
        );
        fs::write(exe(&bin), b"").unwrap();
        assert_eq!(
            find_exe(None, path.clone(), &known_dirs).unwrap(),
            exe(&bin)
        );
        let custom = dir.path().join("custom-app");
        fs::write(&custom, b"").unwrap();
        assert_eq!(
            find_exe(Some(custom.clone().into()), path, &known_dirs).unwrap(),
            custom
        );
    }

    #[test]
    fn missing_app_names_the_override() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_exe(None, Some(dir.path().into()), &[]).unwrap_err();
        assert!(err.hint().contains(APP_ENV));
        let err = find_exe(Some(dir.path().join("nope").into()), None, &[]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains(APP_ENV));
    }

    #[test]
    fn recognizes_the_apps_suffixes_only() {
        assert!(is_instance_of("My Pack", "My Pack"));
        assert!(is_instance_of("My Pack (1)", "My Pack"));
        assert!(is_instance_of("My Pack (12)", "My Pack"));
        assert!(!is_instance_of("My Pack 2", "My Pack"));
        assert!(!is_instance_of("My Pack (x)", "My Pack"));
        assert!(!is_instance_of("Other", "My Pack"));
    }

    /// Stands for the app: records what it opens and, like the user's click,
    /// creates the instance directory after a delay.
    struct FakeLauncher {
        profiles: PathBuf,
        delay: Duration,
        opened: Mutex<Vec<PathBuf>>,
    }

    impl FakeLauncher {
        fn new(profiles: &Path, delay: Duration) -> Self {
            Self {
                profiles: profiles.to_owned(),
                delay,
                opened: Mutex::new(Vec::new()),
            }
        }
    }

    impl Launcher for FakeLauncher {
        fn open(&self, mrpack: &Path) -> Result<()> {
            self.opened.lock().unwrap().push(mrpack.to_owned());
            let name = mrpack.file_stem().unwrap().to_string_lossy().into_owned();
            let (profiles, delay) = (self.profiles.clone(), self.delay);
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let mut dir = profiles.join(&name);
                let mut n = 1;
                while dir.exists() {
                    dir = profiles.join(format!("{name} ({n})"));
                    n += 1;
                }
                fs::create_dir_all(dir).unwrap();
            });
            Ok(())
        }
    }

    #[test]
    fn waits_for_the_directory_the_import_creates() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        let mrpack = dir.path().join("My Pack.mrpack");
        let launcher = FakeLauncher::new(&profiles, Duration::from_millis(300));

        let found = register(&launcher, &profiles, &mrpack, Duration::from_secs(10)).unwrap();
        assert_eq!(found, profiles.join("My Pack"));
        assert_eq!(*launcher.opened.lock().unwrap(), [mrpack]);
    }

    #[test]
    fn finds_the_suffixed_directory_when_the_name_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        fs::create_dir_all(profiles.join("My Pack")).unwrap();
        fs::create_dir_all(profiles.join("My Pack Other")).unwrap();
        let launcher = FakeLauncher::new(&profiles, Duration::from_millis(100));

        let found = register(
            &launcher,
            &profiles,
            &dir.path().join("My Pack.mrpack"),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(found, profiles.join("My Pack (1)"));
    }

    #[test]
    fn gives_up_after_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        fs::create_dir_all(profiles.join("My Pack")).unwrap();
        let start = Instant::now();
        let err = wait_for_instance(
            &profiles,
            "My Pack",
            &instance_dirs(&profiles, "My Pack").unwrap(),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(err.to_string().contains("no instance for `My Pack`"));
        assert!(err.hint().contains("confirm the import"));
    }
}
