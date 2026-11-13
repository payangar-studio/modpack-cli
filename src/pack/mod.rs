//! Building the pack artifacts: one per mode and per platform.
//!
//! Each artifact holds the mods of the resolved mode, and under `overrides/`
//! the base `config/` then the mode's `overrides/<mode>/config/`. Before
//! building for a platform, every mod goes through its `pack_eligibility`; a
//! mod it refuses blocks that platform for that mode only. The platform alone
//! decides, so a mod with no table for a platform (hosted on GitHub, say) may
//! still be packed for it.

mod version;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub use version::{DEV_VERSION, version};

use crate::error::{Context, Error, Result};
use crate::model::manifest::{self, Entry, Manifest, normalize_key};
use crate::model::modes::{self, Modes};
use crate::model::socle::SocleConfig;
use crate::platform::{
    Capability, Eligibility, Operation, Override, PackBuild, Platform, Registry, unsupported,
};

/// Default output directory, relative to the repository root.
pub const OUT_DIR: &str = "dist";

/// What to build.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// One mode; every mode when `None`.
    pub mode: Option<&'a str>,
    /// One platform; every platform that builds packs by code when `None`.
    pub platform: Option<&'a str>,
    pub out_dir: &'a Path,
    pub version: &'a str,
}

/// What was built, and what could not be. Serialized as the `--json` output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub version: String,
    pub artifacts: Vec<Artifact>,
    pub blocked: Vec<Blocked>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Artifact {
    pub mode: String,
    pub platform: String,
    pub path: PathBuf,
}

/// A mode a platform could not pack, and the mods that stopped it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Blocked {
    pub mode: String,
    pub platform: String,
    pub mods: Vec<BlockedMod>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockedMod {
    pub key: String,
    pub reason: String,
}

impl Report {
    /// The error to exit with when a platform was blocked for some mode.
    pub fn blocked_error(&self) -> Option<Error> {
        if self.blocked.is_empty() {
            return None;
        }
        let mut what = "some packs could not be built:".to_owned();
        for b in &self.blocked {
            let _ = write!(what, "\n  - {} pack of mode `{}`:", b.platform, b.mode);
            for m in &b.mods {
                let _ = write!(what, "\n      `{}`: {}", m.key, m.reason);
            }
        }
        Some(Error::blocked(
            what,
            "leave these mods out of the mode in modes.toml, or build only the other platforms with --platform",
        ))
    }
}

/// Builds the artifacts of the repository at `root`.
pub fn build(root: &Path, registry: &Registry, request: &Request<'_>) -> Result<Report> {
    let platforms = platforms(registry, request.platform)?;
    let name = SocleConfig::load(root)?.init.name;
    let manifest = Manifest::load(&root.join(manifest::FILE_NAME))?;
    let modes = Modes::load(&root.join(modes::FILE_NAME))?;
    let mode_names = match request.mode {
        Some(mode) => vec![mode],
        None => modes.names(),
    };
    let base = config_files(&root.join("config"))?;
    fs::create_dir_all(request.out_dir)
        .context(format!("creating {}", request.out_dir.display()))?;
    let pack_slug = Some(normalize_key(&name))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "modpack".to_owned());

    let mut report = Report {
        version: request.version.to_owned(),
        artifacts: Vec::new(),
        blocked: Vec::new(),
    };
    for mode in mode_names {
        let resolved = modes.resolve(mode, &manifest)?;
        let mods: Vec<&Entry> = resolved
            .keys
            .iter()
            .map(|k| {
                manifest
                    .get(k)
                    .expect("a resolved mode only holds manifest keys")
            })
            .collect();
        let mut overrides = base.clone();
        overrides.extend(config_files(
            &root.join("overrides").join(mode).join("config"),
        )?);
        let file_stem = format!("{pack_slug}-{mode}-{}", request.version);

        for platform in &platforms {
            let blocked = ineligible(*platform, &mods)?;
            if !blocked.is_empty() {
                report.blocked.push(Blocked {
                    mode: mode.to_owned(),
                    platform: platform.name().to_owned(),
                    mods: blocked,
                });
                continue;
            }
            let path = platform.build_pack(&PackBuild {
                name: &name,
                version: request.version,
                pack: &manifest.pack,
                mods: mods.clone(),
                overrides: overrides.clone(),
                out_dir: request.out_dir,
                file_stem: &file_stem,
            })?;
            report.artifacts.push(Artifact {
                mode: mode.to_owned(),
                platform: platform.name().to_owned(),
                path,
            });
        }
    }
    Ok(report)
}

/// The platform named on the command line, or every one that builds packs by
/// code.
fn platforms<'r>(registry: &'r Registry, only: Option<&str>) -> Result<Vec<&'r dyn Platform>> {
    let builds = |p: &dyn Platform| p.capability(Operation::BuildPack) == Capability::Api;
    match only {
        Some(name) => {
            let platform = registry.get(name)?;
            if !builds(platform) {
                return Err(unsupported(name, Operation::BuildPack));
            }
            Ok(vec![platform])
        }
        None => Ok(registry.all().filter(|p| builds(*p)).collect()),
    }
}

/// The mods the platform cannot pack, with the reason.
fn ineligible(platform: &dyn Platform, mods: &[&Entry]) -> Result<Vec<BlockedMod>> {
    let mut blocked = Vec::new();
    for entry in mods {
        let reason = match platform.pack_eligibility(entry)? {
            Eligibility::Blocked { reason } => reason,
            Eligibility::Eligible => continue,
        };
        blocked.push(BlockedMod {
            key: entry.key.clone(),
            reason,
        });
    }
    Ok(blocked)
}

/// Every file under `dir`, as overrides under `config/`, sorted by path.
/// `.gitkeep` files only keep empty directories in git and are left out. A
/// missing `dir` gives nothing.
fn config_files(dir: &Path) -> Result<Vec<Override>> {
    let mut files = Vec::new();
    if dir.is_dir() {
        walk(dir, "config", &mut files)?;
    }
    Ok(files)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Override>) -> Result<()> {
    let reading = || format!("reading {}", dir.display());
    let mut entries = fs::read_dir(dir)
        .context(reading())?
        .collect::<std::io::Result<Vec<_>>>()
        .context(reading())?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let source = entry.path();
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            return Err(Error::usage(
                format!("{} has a name that is not valid UTF-8", source.display()),
                "rename it",
            ));
        };
        let path = format!("{prefix}/{file_name}");
        if source.is_dir() {
            walk(&source, &path, out)?;
        } else if file_name != ".gitkeep" {
            out.push(Override { source, path });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
