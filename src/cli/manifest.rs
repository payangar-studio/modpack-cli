//! `modpack manifest`: adds the new files of mods/, resourcepacks/ and their
//! local directories to the manifest, rebuilds the entries whose file changed
//! and links entries to the platforms that did not know them yet. A missing
//! file keeps its entry.
//! Then checks `variants.toml` against the manifest, so that the `pre-commit`
//! hook, which runs this command, stops a commit whose variants are broken.

use std::io::Write;
use std::path::Path;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::model::manifest::{FILE_NAME, Manifest};
use crate::model::variants::{self, Variants};
use crate::ops::manifest::{check, regenerate};
use crate::platform::Registry;

#[derive(Debug, clap::Args)]
#[command(
    after_help = "A file missing from mods/ or resourcepacks/ means \"not installed here\": its entry is kept, with a warning. `modpack install` downloads it; `modpack remove <key>` takes it out of the pack and its variants."
)]
pub struct Args {
    /// Fail if the manifest is out of date instead of rewriting it
    #[arg(long)]
    pub check: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    update_or_check(args, root)?;
    check_variants(root)
}

/// `variants.toml` parses and names only keys of the manifest. A repository
/// without `variants.toml` has nothing to check.
fn check_variants(root: &Path) -> Result<()> {
    let path = root.join(variants::FILE_NAME);
    if !path.is_file() {
        return Ok(());
    }
    Variants::load(&path)?.check(&Manifest::load(&root.join(FILE_NAME))?)
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
                "{FILE_NAME} does not match the repository's files ({} difference(s))",
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
        // Git holds the local files: `modpack install` cannot restore them.
        let manifest = Manifest::load(&root.join(FILE_NAME))?;
        let (local, missing): (Vec<String>, Vec<String>) = changes
            .missing
            .iter()
            .cloned()
            .partition(|k| manifest.get(k).is_some_and(|e| e.is_local()));
        if !missing.is_empty() {
            writeln!(
                out,
                "warning: file missing, entry kept (`modpack install` downloads it, `modpack remove <key>` removes it): {}",
                missing.join(", ")
            )?;
        }
        if !local.is_empty() {
            writeln!(
                out,
                "warning: local file missing, entry kept (`git checkout -- <path>` restores it, `modpack remove <key>` removes it): {}",
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
        ("rebuilt, file changed", &changes.rebuilt),
        ("linked to more platforms", &changes.linked),
    ] {
        if !keys.is_empty() {
            writeln!(out, "{label}: {}", keys.join(", "))?;
        }
    }
    if !changes.unidentified.is_empty() {
        writeln!(
            out,
            "warning: found on no platform, give their direct URL with `modpack add <path> --url <url>`, or move their file to mods-local/ or resourcepacks-local/ to track it with git: {}",
            changes.unidentified.join(", ")
        )?;
    }
    if !changes.unapproved.is_empty() {
        writeln!(
            out,
            "local files not approved for publication, review each one then run `modpack approve <key>`: {}",
            changes.unapproved.join(", ")
        )?;
    }
    Ok(())
}
