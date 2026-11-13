//! The machine side of a pack: which Modrinth instance belongs to which pack,
//! and what the CLI last wrote into that instance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// File of the instance's state, at the root of the instance directory.
pub const STATE_FILE: &str = ".modpack-state.toml";

const INSTANCES_HEADER: &str =
    "# Written by modpack-cli: the Modrinth instance of each pack on this machine.\n\n";
const STATE_HEADER: &str =
    "# Written by modpack-cli: what it last wrote into this instance. Do not edit by hand.\n\n";

/// `<user config dir>/modpack/instances.toml`: pack name to instance directory.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instances {
    #[serde(default)]
    pub instances: BTreeMap<String, PathBuf>,
}

impl Instances {
    /// Where the file lives on this machine.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new().ok_or_else(|| {
            Error::internal(
                "cannot find the user's home directory",
                "set HOME to the user's home directory",
            )
        })?;
        Ok(dirs.config_dir().join("modpack").join("instances.toml"))
    }

    /// Reads the file; a missing file is a machine with no instance yet.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        super::parse_toml(
            &super::read_file(path)?,
            &path.display().to_string(),
            "fix or delete the file, then run `modpack setup` again",
        )
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        super::write_file(path, &format!("{INSTANCES_HEADER}{}", super::to_toml(self)))
    }

    /// The pack's instance on this machine: registered in `path` and still
    /// present. An instance deleted in the app counts as none.
    pub fn existing(path: &Path, pack: &str) -> Result<Option<PathBuf>> {
        Ok(Self::load(path)?
            .get(pack)
            .filter(|dir| dir.is_dir())
            .map(Path::to_path_buf))
    }

    pub fn get(&self, pack: &str) -> Option<&Path> {
        self.instances.get(pack).map(PathBuf::as_path)
    }

    pub fn set(&mut self, pack: &str, instance_dir: PathBuf) {
        self.instances.insert(pack.to_owned(), instance_dir);
    }
}

/// `.modpack-state.toml` of an instance: the loaded mode and the sha256 of
/// every file the CLI wrote there, the reference of drift detection.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceState {
    /// Mode loaded last; `None` before the first load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Path relative to the instance directory, with `/` separators, to sha256.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl InstanceState {
    /// Reads the state of an instance; a missing file is an instance the CLI
    /// never wrote to.
    pub fn load(instance_dir: &Path) -> Result<Self> {
        let path = instance_dir.join(STATE_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        super::parse_toml(
            &super::read_file(&path)?,
            &path.display().to_string(),
            "delete the file, then load a mode with `modpack mode load <mode> --force`",
        )
    }

    pub fn save(&self, instance_dir: &Path) -> Result<()> {
        super::write_file(
            &instance_dir.join(STATE_FILE),
            &format!("{STATE_HEADER}{}", super::to_toml(self)),
        )
    }

    /// Records a file the CLI just wrote, by hashing it as it is now.
    pub fn record(&mut self, instance_dir: &Path, relative: &str) -> Result<()> {
        let hash = hash(&instance_dir.join(relative))?;
        self.files.insert(relative.to_owned(), hash);
        Ok(())
    }

    /// Forgets a file the CLI removed.
    pub fn forget(&mut self, relative: &str) {
        self.files.remove(relative);
    }
}

/// Files of `state` changed or gone since the CLI wrote them, relative to the
/// instance directory, sorted.
pub fn drift(instance_dir: &Path, state: &InstanceState) -> Result<Vec<PathBuf>> {
    let mut drifted = Vec::new();
    for (relative, expected) in &state.files {
        let path = instance_dir.join(relative);
        let changed = match std::fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            _ => hash(&path)? != *expected,
        };
        if changed {
            drifted.push(PathBuf::from(relative));
        }
    }
    Ok(drifted)
}

fn hash(path: &Path) -> Result<String> {
    super::sha256_file(path).map_err(|e| {
        Error::internal(
            format!("cannot read {}", path.display()),
            "check the file's permissions",
        )
        .with_source(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// An instance with two files the CLI wrote, and its state.
    fn instance() -> (tempfile::TempDir, InstanceState) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("mods")).unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("mods/jei.jar"), b"jar").unwrap();
        fs::write(dir.path().join("config/jei.toml"), b"a = 1").unwrap();
        let mut state = InstanceState {
            mode: Some("default".into()),
            ..Default::default()
        };
        state.record(dir.path(), "mods/jei.jar").unwrap();
        state.record(dir.path(), "config/jei.toml").unwrap();
        (dir, state)
    }

    #[test]
    fn clean_instance_has_no_drift() {
        let (dir, state) = instance();
        fs::write(dir.path().join("mods/unknown.jar"), b"tried in the app").unwrap();
        assert_eq!(drift(dir.path(), &state).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn modified_file_drifts() {
        let (dir, state) = instance();
        fs::write(dir.path().join("config/jei.toml"), b"a = 2").unwrap();
        assert_eq!(
            drift(dir.path(), &state).unwrap(),
            [PathBuf::from("config/jei.toml")]
        );
    }

    #[test]
    fn deleted_file_drifts() {
        let (dir, state) = instance();
        fs::remove_file(dir.path().join("mods/jei.jar")).unwrap();
        assert_eq!(
            drift(dir.path(), &state).unwrap(),
            [PathBuf::from("mods/jei.jar")]
        );
    }

    #[test]
    fn forgotten_file_no_longer_drifts() {
        let (dir, mut state) = instance();
        fs::remove_file(dir.path().join("mods/jei.jar")).unwrap();
        state.forget("mods/jei.jar");
        assert!(drift(dir.path(), &state).unwrap().is_empty());
    }

    #[test]
    fn state_round_trips_and_defaults_when_missing() {
        let (dir, state) = instance();
        assert_eq!(
            InstanceState::load(dir.path()).unwrap(),
            InstanceState::default()
        );
        state.save(dir.path()).unwrap();
        assert_eq!(InstanceState::load(dir.path()).unwrap(), state);
        let text = fs::read_to_string(dir.path().join(STATE_FILE)).unwrap();
        assert!(text.contains("mode = \"default\""));
        assert!(text.contains("\"mods/jei.jar\" = "));
    }

    #[test]
    fn instances_round_trip_and_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("modpack/instances.toml");
        let mut instances = Instances::load(&path).unwrap();
        assert!(instances.get("My Pack").is_none());
        instances.set("My Pack", dir.path().join("profiles/My Pack"));
        instances.save(&path).unwrap();
        let loaded = Instances::load(&path).unwrap();
        assert_eq!(
            loaded.get("My Pack"),
            Some(dir.path().join("profiles/My Pack").as_path())
        );
    }

    #[test]
    fn missing_or_deleted_instance_counts_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("instances.toml");
        assert_eq!(Instances::existing(&file, "P").unwrap(), None);
        let instance = dir.path().join("profiles/P");
        let mut instances = Instances::default();
        instances.set("P", instance.clone());
        instances.save(&file).unwrap();
        assert_eq!(Instances::existing(&file, "P").unwrap(), None);
        fs::create_dir_all(&instance).unwrap();
        assert_eq!(Instances::existing(&file, "P").unwrap(), Some(instance));
    }

    #[test]
    fn instances_file_is_in_the_user_config_dir() {
        let path = Instances::default_path().unwrap();
        assert!(path.ends_with("modpack/instances.toml"));
        let config = directories::BaseDirs::new()
            .unwrap()
            .config_dir()
            .to_owned();
        assert!(path.starts_with(config));
    }
}
