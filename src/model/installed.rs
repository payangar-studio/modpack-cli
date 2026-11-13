//! `.modpack/local/installed.toml`: the jars the CLI downloaded into the
//! repository's `mods/` on this machine. Ignored by git.
//!
//! A jar recorded here, still as the CLI wrote it, is the CLI's: once the
//! manifest no longer lists it, `modpack install` removes it and `modpack
//! manifest` never adds it back. Any other jar of `mods/` is the user's.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// The record, relative to the repository root.
pub const FILE: &str = ".modpack/local/installed.toml";

const HEADER: &str = "# Written by modpack-cli: the jars it downloaded into mods/ on this machine. Do not edit by hand.\n\n";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    /// `mods/<name>` to the sha1 of the jar the CLI wrote there.
    #[serde(default)]
    pub jars: BTreeMap<String, String>,
}

impl Installed {
    /// Reads the record; a missing file is a repository where the CLI never
    /// downloaded a jar.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        super::parse_toml(
            &super::read_file(&path)?,
            &path.display().to_string(),
            "delete the file, then run `modpack install` again",
        )
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        super::write_file(
            &root.join(FILE),
            &format!("{HEADER}{}", super::to_toml(self)),
        )
    }

    /// Whether the CLI wrote the jar at `path` with this sha1.
    pub fn placed(&self, path: &str, sha1: &str) -> bool {
        self.jars.get(path).is_some_and(|s| s == sha1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_a_missing_file_is_empty() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(Installed::load(root.path()).unwrap(), Installed::default());
        let mut installed = Installed::default();
        installed.jars.insert("mods/jei.jar".into(), "abc".into());
        installed.save(root.path()).unwrap();
        let loaded = Installed::load(root.path()).unwrap();
        assert_eq!(loaded, installed);
        assert!(loaded.placed("mods/jei.jar", "abc"));
        assert!(!loaded.placed("mods/jei.jar", "def"));
        assert!(!loaded.placed("mods/iris.jar", "abc"));
    }
}
