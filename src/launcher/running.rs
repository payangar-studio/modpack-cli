//! Whether Minecraft runs from an instance.
//!
//! Read in the app's code (`app-lib`, `launcher/args.rs`): the app starts a
//! Java process whose command line holds the instance directory, canonical,
//! as `--gameDir`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::error::{Error, Result};

/// Tells whether Minecraft runs from an instance. Commands take it as a
/// parameter so that their tests can pass a fake.
pub trait GameDetector {
    fn is_running(&self, instance_dir: &Path) -> Result<bool>;
}

/// Looks at the processes of this machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct Processes;

impl GameDetector for Processes {
    fn is_running(&self, instance_dir: &Path) -> Result<bool> {
        let mut system = System::new().map_err(cannot_list)?;
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        );
        let dirs = variants(instance_dir);
        Ok(system
            .processes()
            .values()
            .any(|p| runs_from(p.name(), p.cmd(), &dirs)))
    }
}

fn cannot_list(e: sysinfo::Error) -> Error {
    Error::internal(
        "cannot list the running processes to check whether Minecraft runs",
        "close Minecraft, then rerun the command",
    )
    .with_source(e)
}

/// The directory as given and canonical, since the app passes the latter.
fn variants(dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![dir.to_owned()];
    if let Ok(canonical) = dir.canonicalize()
        && canonical != dir
    {
        dirs.push(canonical);
    }
    dirs
}

/// A Java process (`java`, `javaw`, with or without `.exe`) with an argument
/// that is one of `dirs` or a path inside it.
fn runs_from(name: &OsStr, cmd: &[OsString], dirs: &[PathBuf]) -> bool {
    let is_java = name
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with("java");
    is_java
        && cmd
            .iter()
            .any(|arg| dirs.iter().any(|d| Path::new(arg).starts_with(d)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn matches_a_java_process_of_the_instance_only() {
        let dirs = [PathBuf::from("/data/profiles/My Pack")];
        let game = args(&["java", "-Xmx4G", "--gameDir", "/data/profiles/My Pack"]);
        assert!(runs_from(OsStr::new("java"), &game, &dirs));
        assert!(runs_from(OsStr::new("javaw.exe"), &game, &dirs));
        assert!(!runs_from(OsStr::new("vim"), &game, &dirs));

        let other = args(&["java", "--gameDir", "/data/profiles/My Pack (1)"]);
        assert!(!runs_from(OsStr::new("java"), &other, &dirs));
        let inside = args(&["java", "/data/profiles/My Pack/mods/a.jar"]);
        assert!(runs_from(OsStr::new("java"), &inside, &dirs));
    }

    /// Starts a real process named `java` whose command line holds the
    /// instance directory, and checks the system detector sees it.
    #[cfg(target_os = "linux")]
    #[test]
    fn detects_a_fake_minecraft_process() {
        use std::process::Command;
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let instance = dir.path().join("profiles/My Pack");
        let other = dir.path().join("profiles/Other");
        std::fs::create_dir_all(&instance).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let java = dir.path().join("java");
        std::os::unix::fs::symlink("/bin/sh", &java).unwrap();
        // `; :` keeps the shell from replacing itself with `sleep`.
        let mut child = Command::new(&java)
            .args(["-c", "sleep 30; :", "sh", "--gameDir"])
            .arg(&instance)
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = false;
        while !seen && Instant::now() < deadline {
            seen = Processes.is_running(&instance).unwrap();
            if !seen {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let other_seen = Processes.is_running(&other).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();

        assert!(seen, "the fake Minecraft process was not detected");
        assert!(!other_seen);
        assert!(!Processes.is_running(&instance).unwrap());
    }
}
