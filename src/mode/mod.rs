//! Loading a mode into the pack's Modrinth instance.
//!
//! The repository feeds the instance one way. Loading a mode, in this order:
//! finds the pack's instance, refuses while Minecraft runs from it, stops on
//! drift unless forced, resolves the mode, then puts the mode's jars in the
//! instance's `mods/`, writes the base config and the mode's overrides into
//! its `config/`, removes the config files it wrote that the mode does not
//! have, and records what it wrote in `.modpack-state.toml`.
//!
//! A jar of the instance is identified by its hashes. Only jars the manifest
//! knows are ever removed; any other jar is the user's, tried from the app,
//! and is never touched.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Context, Error, Result};
use crate::launcher::running::GameDetector;
use crate::model::instance::{self, DISABLED_SUFFIX, InstanceState, Instances, disabled_in_app};
use crate::model::manifest::{self, Entry, Manifest};
use crate::model::modes::{self, Modes};
use crate::model::sha256_file;
use crate::model::socle::SocleConfig;
use crate::ops::local::restore_hint;
use crate::store::download::{Sources, download_all};
use crate::store::inventory::inventory;
use crate::store::{MODS_DIR, jar_path, jars_in, write_atomic};

/// Base config, in the repository and in the instance.
pub const CONFIG_DIR: &str = "config";
/// `overrides/<mode>/config/`: the config files a mode overrides.
pub const OVERRIDES_DIR: &str = "overrides";
/// Placeholder that keeps an empty directory in git; never copied.
const GITKEEP: &str = ".gitkeep";

/// What loading a mode needs from the machine. Commands build it from the
/// real machine; tests pass temporary directories and fakes.
#[derive(Clone, Copy)]
pub struct LoadContext<'a> {
    /// Root of the modpack repository.
    pub root: &'a Path,
    /// The machine's `instances.toml`.
    pub instances_file: &'a Path,
    /// Tells whether Minecraft runs from the instance.
    pub game: &'a dyn GameDetector,
    /// Where jars missing from the repository's `mods/` are downloaded from.
    pub sources: Sources<'a>,
}

/// What a load did. Paths are relative to the instance, with `/` separators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub mode: String,
    pub instance: PathBuf,
    /// Keys of the mode's mods downloaded into the repository's `mods/`.
    pub downloaded: Vec<String>,
    /// Jars copied into the instance.
    pub copied: Vec<String>,
    /// Jars of the manifest outside the mode, removed from the instance.
    pub removed: Vec<String>,
    /// Jars unknown to the manifest, left as they are.
    pub unknown: Vec<String>,
    /// Config files written into the instance: new, or with other content.
    pub written: Vec<String>,
    /// Config files the CLI wrote for another mode, removed from the instance.
    pub config_removed: Vec<String>,
    /// Files changed in the instance that `--force` overwrote or let go.
    pub forced: Vec<String>,
}

/// Loads `mode` into the pack's instance. `force` goes past files of the
/// instance changed since the CLI wrote them, and past foreign jars sitting
/// where a jar of the mode goes.
pub fn load(ctx: &LoadContext<'_>, mode: &str, force: bool) -> Result<LoadReport> {
    let root = ctx.root;
    // The mode first: an unknown mode is reported before a missing instance.
    let manifest = load_manifest(root)?;
    let resolved = load_modes(root)?.resolve(mode, &manifest)?;
    let instance = find_instance(root, ctx.instances_file)?;
    if ctx.game.is_running(&instance)? {
        return Err(Error::blocked(
            format!("Minecraft is running from {}", instance.display()),
            "quit Minecraft, then load the mode again",
        ));
    }

    let state = InstanceState::load(&instance)?;
    let drifted: Vec<String> = instance::drift(&instance, &state)?
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut disabled = Vec::new();
    for rel in &drifted {
        if disabled_in_app(&instance, &state, rel)? {
            disabled.push(rel.clone());
        }
    }
    if !drifted.is_empty() && !force {
        let described: Vec<String> = drifted
            .iter()
            .map(|rel| match disabled.contains(rel) {
                true => format!("{rel} (disabled in the app)"),
                false => rel.clone(),
            })
            .collect();
        return Err(Error::blocked(
            format!(
                "{} file(s) of the instance changed since modpack-cli wrote them:{}",
                drifted.len(),
                bullets(&described)
            ),
            "bring the changes you want to keep into the repository, then rerun with --force to overwrite them",
        ));
    }

    let wanted: Vec<&Entry> = resolved
        .keys
        .iter()
        .filter_map(|k| manifest.get(k))
        .collect();
    let in_mode: BTreeSet<&str> = resolved.keys.iter().map(String::as_str).collect();

    // Sort the instance's jars: the mode's, other known ones, unknown ones.
    let mut new_state = InstanceState {
        mode: Some(mode.to_owned()),
        files: BTreeMap::new(),
    };
    let mut present = BTreeSet::new();
    let mut to_remove = Vec::new();
    let mut unknown = Vec::new();
    for (rel, hashes) in jars_in(&instance.join(MODS_DIR))? {
        match manifest.mods.iter().find(|e| hashes.matches(&e.hashes)) {
            Some(e) if in_mode.contains(e.key.as_str()) => {
                present.insert(e.key.as_str());
                new_state.files.insert(rel, hashes.sha256);
            }
            Some(_) => to_remove.push(rel),
            // A jar the CLI wrote, unchanged since, whose mod the manifest
            // dropped or updated: the CLI's to remove, never the user's.
            None if state.files.get(&rel) == Some(&hashes.sha256) => to_remove.push(rel),
            None => unknown.push(rel),
        }
    }
    let mut to_copy = Vec::new();
    for entry in wanted.iter().filter(|e| !present.contains(e.key.as_str())) {
        to_copy.push((jar_path(root, entry)?, entry.instance_path()));
    }

    // A foreign file where a jar of the mode goes is never overwritten silently.
    let in_the_way: Vec<String> = to_copy
        .iter()
        .map(|(_, rel)| rel.clone())
        .filter(|rel| instance.join(rel).exists() && !to_remove.contains(rel))
        .collect();
    if !in_the_way.is_empty() && !force {
        return Err(Error::blocked(
            format!(
                "{} jar(s) of the instance unknown to the manifest sit where a jar of the mode goes:{}",
                in_the_way.len(),
                bullets(&in_the_way)
            ),
            "move them out of the instance's mods/, or rerun with --force to overwrite them",
        ));
    }
    unknown.retain(|rel| !in_the_way.contains(rel));

    // Nor is a config file of the instance the CLI never wrote, such as one
    // the game created, when the mode has other content for it.
    let configs = config_files(root, mode)?;
    let mut foreign = Vec::new();
    for (rel, source) in &configs {
        let dest = instance.join(rel);
        if !state.files.contains_key(rel) && dest.is_file() && !same_content(source, &dest)? {
            foreign.push(rel.clone());
        }
    }
    if !foreign.is_empty() && !force {
        return Err(Error::blocked(
            format!(
                "{} config file(s) of the instance that modpack-cli did not write differ from the mode's:{}",
                foreign.len(),
                bullets(&foreign)
            ),
            "bring the changes you want to keep into the repository, then rerun with --force to overwrite them",
        ));
    }

    let downloaded = fill_stock(root, &manifest, &wanted, ctx.sources)?;

    let mut report = LoadReport {
        mode: mode.to_owned(),
        instance: instance.clone(),
        downloaded,
        unknown,
        forced: drifted
            .into_iter()
            .chain(in_the_way)
            .chain(foreign)
            .collect(),
        ..Default::default()
    };
    report.forced.sort();
    report.forced.dedup();

    for rel in to_remove {
        let path = instance.join(&rel);
        fs::remove_file(&path).context(format!("removing {}", path.display()))?;
        report.removed.push(rel);
    }
    for (source, rel) in to_copy {
        copy_file(&source, &instance.join(&rel))?;
        new_state.record(&instance, &rel)?;
        report.copied.push(rel);
    }
    // Forced past a jar the app disabled: the repository wins, so the jar is
    // back (when the mode has it) and its disabled copy goes.
    for rel in disabled {
        let path = instance.join(format!("{rel}{DISABLED_SUFFIX}"));
        fs::remove_file(&path).context(format!("removing {}", path.display()))?;
    }

    for (rel, source) in &configs {
        let dest = instance.join(rel);
        if !(dest.is_file() && same_content(source, &dest)?) {
            copy_file(source, &dest)?;
            report.written.push(rel.clone());
        }
        new_state.record(&instance, rel)?;
    }
    // A config file the CLI wrote that this mode does not have, such as the
    // previous mode's override, goes. Changed since, it is drift: past the
    // check above only with --force, which lets the repository win.
    let config_prefix = format!("{CONFIG_DIR}/");
    for rel in state.files.keys() {
        if !rel.starts_with(&config_prefix) || configs.contains_key(rel) {
            continue;
        }
        let path = instance.join(rel);
        match fs::remove_file(&path) {
            Ok(()) => report.config_removed.push(rel.clone()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context(format!("removing {}", path.display())),
        }
    }

    new_state.save(&instance)?;
    Ok(report)
}

/// The mode loaded in the pack's instance on this machine, if the pack has an
/// instance here and a mode was loaded into it.
pub fn loaded_mode(root: &Path, instances_file: &Path) -> Result<Option<String>> {
    let name = SocleConfig::load(root)?.init.name;
    match Instances::existing(instances_file, &name)? {
        Some(dir) => Ok(InstanceState::load(&dir)?.mode),
        None => Ok(None),
    }
}

/// Reads `modes.toml` at the repository root.
pub fn load_modes(root: &Path) -> Result<Modes> {
    let path = root.join(modes::FILE_NAME);
    if !path.is_file() {
        return Err(Error::usage(
            format!("no {} in this repository", modes::FILE_NAME),
            format!(
                "create {} at the repository root, with at least a [default] table",
                modes::FILE_NAME
            ),
        ));
    }
    Modes::load(&path)
}

fn load_manifest(root: &Path) -> Result<Manifest> {
    let path = root.join(manifest::FILE_NAME);
    if !path.is_file() {
        return Err(Error::usage(
            format!("no {} in this repository", manifest::FILE_NAME),
            "generate it with `modpack manifest`",
        ));
    }
    Manifest::load(&path)
}

fn find_instance(root: &Path, instances_file: &Path) -> Result<PathBuf> {
    let name = SocleConfig::load(root)?.init.name;
    Instances::existing(instances_file, &name)?.ok_or_else(|| {
        Error::usage(
            format!("the pack `{name}` has no Modrinth instance on this machine"),
            "run `modpack setup` to create it",
        )
    })
}

/// Makes sure the repository holds every jar of the mode, with the
/// manifest's hashes, downloading the missing ones into `mods/`. Returns their
/// keys. A local mod's jar is never downloaded: git holds it.
fn fill_stock(
    root: &Path,
    manifest: &Manifest,
    wanted: &[&Entry],
    sources: Sources<'_>,
) -> Result<Vec<String>> {
    let inv = inventory(root, manifest)?;
    let local = |key: &String| manifest.get(key).is_some_and(Entry::is_local);
    // Git holds the jars of mods-local/: neither a download nor
    // `modpack install` restores them.
    let local_gaps: Vec<(&Entry, &str)> = [(&inv.missing, "missing"), (&inv.mismatched, "changed")]
        .into_iter()
        .flat_map(|(keys, how)| keys.iter().map(move |k| (k, how)))
        .filter(|(k, _)| local(k))
        .filter_map(|(k, how)| wanted.iter().find(|e| &e.key == k).map(|e| (*e, how)))
        .collect();
    if !local_gaps.is_empty() {
        let lines: Vec<String> = local_gaps
            .iter()
            .map(|(e, how)| format!("{} ({}), {how}", e.key, e.path))
            .collect();
        let entries: Vec<&Entry> = local_gaps.iter().map(|(e, _)| *e).collect();
        return Err(Error::blocked(
            format!(
                "local jar(s) of the mode missing from mods-local/ or changed since the manifest:{}",
                bullets(&lines)
            ),
            format!(
                "{}; run `modpack manifest` instead for a jar you changed on purpose",
                restore_hint(&entries)
            ),
        ));
    }
    let in_mode = |keys: &[String]| -> Vec<String> {
        keys.iter()
            .filter(|k| !local(k) && wanted.iter().any(|e| &e.key == *k))
            .cloned()
            .collect()
    };
    let mismatched = in_mode(&inv.mismatched);
    if !mismatched.is_empty() {
        return Err(Error::blocked(
            format!(
                "jar(s) of the mode in the repository's mods/ differ from the manifest: {}",
                mismatched.join(", ")
            ),
            "restore them with `modpack install --force`, or run `modpack manifest` if you changed them on purpose",
        ));
    }
    let missing = in_mode(&inv.missing);
    let entries: Vec<&Entry> = missing.iter().filter_map(|k| manifest.get(k)).collect();
    let failed: Vec<String> = download_all(root, &entries, sources)?
        .into_iter()
        .filter_map(|r| r.err().map(|f| f.key))
        .collect();
    if !failed.is_empty() {
        return Err(Error::internal(
            format!(
                "{} jar(s) of the mode could not be downloaded: {}",
                failed.len(),
                failed.join(", ")
            ),
            "run `modpack install` to see why each source failed",
        ));
    }
    Ok(missing)
}

/// The config files to write, as `config/<path>` to their source: the base
/// config, then the mode's overrides, which win on the files they share.
fn config_files(root: &Path, mode: &str) -> Result<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    walk(&root.join(CONFIG_DIR), CONFIG_DIR, &mut files)?;
    let overrides = root.join(OVERRIDES_DIR).join(mode).join(CONFIG_DIR);
    walk(&overrides, CONFIG_DIR, &mut files)?;
    Ok(files)
}

/// Adds every file under `dir` to `files`, as `<prefix>/<path>`. A missing
/// directory adds nothing.
fn walk(dir: &Path, prefix: &str, files: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context(format!("reading {}", dir.display())),
    };
    for item in listing {
        let item = item.context(format!("reading {}", dir.display()))?;
        let path = item.path();
        let name = item.file_name();
        let name = name.to_str().ok_or_else(|| {
            Error::usage(
                format!("{} is not a UTF-8 file name", path.display()),
                "rename it",
            )
        })?;
        let rel = format!("{prefix}/{name}");
        if path.is_dir() {
            walk(&path, &rel, files)?;
        } else if name != GITKEEP {
            files.insert(rel, path);
        }
    }
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> Result<bool> {
    let hash = |p: &Path| sha256_file(p).context(format!("reading {}", p.display()));
    Ok(hash(a)? == hash(b)?)
}

/// Copies through a temporary file then a rename, so that a crash never
/// leaves a half-written file in the instance.
fn copy_file(source: &Path, dest: &Path) -> Result<()> {
    let mut from = fs::File::open(source).context(format!("reading {}", source.display()))?;
    write_atomic(dest, |file| {
        io::copy(&mut from, file).context(format!("writing {}", dest.display()))?;
        Ok(())
    })
}

fn bullets(items: &[String]) -> String {
    items.iter().map(|i| format!("\n  - {i}")).collect()
}

/// The report as shown after `modpack mode load` and `modpack install`.
impl fmt::Display for LoadReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "mode `{}` loaded into {}",
            self.mode,
            self.instance.display()
        )?;
        let lines = [
            ("downloaded into the repository", &self.downloaded),
            ("jars copied", &self.copied),
            ("jars removed", &self.removed),
            ("config written", &self.written),
            ("config removed", &self.config_removed),
            (
                "changed in the instance, overwritten (--force)",
                &self.forced,
            ),
            ("not in the manifest, left as is", &self.unknown),
        ];
        // Everything but the jars left as they are.
        if lines[..lines.len() - 1]
            .iter()
            .all(|(_, items)| items.is_empty())
        {
            writeln!(f, "nothing changed in the instance")?;
        }
        for (label, items) in lines {
            if !items.is_empty() {
                writeln!(f, "{label}: {}", items.join(", "))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
