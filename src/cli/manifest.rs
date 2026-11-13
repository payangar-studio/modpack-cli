//! `modpack manifest`: adds the new jars of mods/ to the manifest and
//! rebuilds the entries whose jar changed. A missing jar keeps its entry.

use std::io::Write;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::model::manifest::FILE_NAME;
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
                "{FILE_NAME} does not match mods/ ({} difference(s))",
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
        writeln!(
            out,
            "warning: jar missing from mods/, entry kept (`modpack install` downloads it, `modpack remove <key>` removes the mod): {}",
            changes.missing.join(", ")
        )?;
    }
    if changes.is_empty() {
        writeln!(out, "{FILE_NAME} is up to date")?;
        return Ok(());
    }
    for (label, keys) in [
        ("added", &changes.added),
        ("rebuilt, jar changed", &changes.rebuilt),
    ] {
        if !keys.is_empty() {
            writeln!(out, "{label}: {}", keys.join(", "))?;
        }
    }
    if !changes.unidentified.is_empty() {
        writeln!(
            out,
            "warning: found on no platform, give their direct URL with `modpack add mods/<jar> --url <url>`: {}",
            changes.unidentified.join(", ")
        )?;
    }
    Ok(())
}
