//! `modpack manifest`: adds the new jars of mods/ and mods-local/ to the
//! manifest, rebuilds
//! the entries whose jar changed and links entries to the platforms that did
//! not know them yet. A missing jar keeps its entry.
//! Then checks `modes.toml` against the manifest, so that the `pre-commit`
//! hook, which runs this command, stops a commit whose modes are broken.

use std::io::Write;
use std::path::Path;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::model::manifest::{FILE_NAME, Manifest};
use crate::model::modes::{self, Modes};
use crate::ops::manifest::{check, regenerate};
use crate::platform::Registry;

#[derive(Debug, clap::Args)]
#[command(
    after_help = "A jar missing from mods/ means \"not installed here\": its entry is kept, with a warning. `modpack install` downloads it; `modpack remove <key>` takes the mod out of the pack and its modes."
)]
pub struct Args {
    /// Fail if the manifest is out of date instead of rewriting it
    #[arg(long)]
    pub check: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    update_or_check(args, root)?;
    check_modes(root)
}

/// `modes.toml` parses and names only keys of the manifest. A repository
/// without `modes.toml` has nothing to check.
fn check_modes(root: &Path) -> Result<()> {
    let path = root.join(modes::FILE_NAME);
    if !path.is_file() {
        return Ok(());
    }
    Modes::load(&path)?.check(&Manifest::load(&root.join(FILE_NAME))?)
}

fn update_or_check(args: Args, root: &Path) -> Result<()> {
    let out = &mut std::io::stdout().lock();
    if args.check {
        let gaps = check(root)?;
        if gaps.is_empty() {
            writeln!(out, "{FILE_NAME} is up to date")?;
            return Ok(());
        }
        for gap in &gaps {
            writeln!(out, "{gap}")?;
        }
        return Err(Error::blocked(
            format!(
                "{FILE_NAME} does not match mods/ and mods-local/ ({} difference(s))",
                gaps.len()
            ),
            format!("run `modpack manifest`, then commit {FILE_NAME}"),
        ));
    }

    let changes = regenerate(root, &Registry::builtin())?;
    for warning in &changes.warnings {
        writeln!(out, "warning: {warning}")?;
    }
    if !changes.missing.is_empty() {
        // Git holds the jars of mods-local/: `modpack install` cannot restore
        // them.
        let manifest = Manifest::load(&root.join(FILE_NAME))?;
        let (local, missing): (Vec<String>, Vec<String>) = changes
            .missing
            .iter()
            .cloned()
            .partition(|k| manifest.get(k).is_some_and(|e| e.is_local()));
        if !missing.is_empty() {
            writeln!(
                out,
                "warning: jar missing from mods/, entry kept (`modpack install` downloads it, `modpack remove <key>` removes the mod): {}",
                missing.join(", ")
            )?;
        }
        if !local.is_empty() {
            writeln!(
                out,
                "warning: local jar missing from mods-local/, entry kept (`git checkout -- mods-local/<file>` restores it, `modpack remove <key>` removes the mod): {}",
                local.join(", ")
            )?;
        }
    }
    if changes.is_empty() {
        writeln!(out, "{FILE_NAME} is up to date")?;
        return Ok(());
    }
    for (label, keys) in [
        ("added", &changes.added),
        ("rebuilt, jar changed", &changes.rebuilt),
        ("linked to more platforms", &changes.linked),
    ] {
        if !keys.is_empty() {
            writeln!(out, "{label}: {}", keys.join(", "))?;
        }
    }
    if !changes.unidentified.is_empty() {
        writeln!(
            out,
            "warning: found on no platform, give their direct URL with `modpack add mods/<jar> --url <url>`, or move their jar to mods-local/ to track it with git: {}",
            changes.unidentified.join(", ")
        )?;
    }
    if !changes.unapproved.is_empty() {
        writeln!(
            out,
            "local mods not approved for publication, review each jar then run `modpack approve <key>`: {}",
            changes.unapproved.join(", ")
        )?;
    }
    Ok(())
}
