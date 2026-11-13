//! `modpack update`: brings the repository's socle to this CLI's version.
//!
//! 1. Compares the repository's socle version to the CLI's; stops when equal,
//!    unless `--force`.
//! 2. Checks every file and block of the registry against its recorded hash;
//!    stops on hand edits unless `--force`.
//! 3. Writes the socle files and blocks, removes those that left the socle.
//! 4. Rewrites the registry and the version.

use crate::context::Context;
use crate::error::{Error, Result};
use crate::model::socle::{SocleConfig, SocleLock, VersionCheck, cli_version, compare_versions};
use crate::socle;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Overwrite socle files edited by hand
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root_unchecked()?;
    let mut config = SocleConfig::load(root)?;
    let cli = cli_version();
    let check = compare_versions(&config.version, &cli)?;
    match check {
        // `--force` rewrites the socle at the same version: a file deleted or
        // edited by hand, or a broken registry.
        VersionCheck::Equal if !args.force => {
            println!("The socle is already at version {cli}: nothing to update.");
            return Ok(());
        }
        VersionCheck::Equal => {}
        VersionCheck::RepoNewer { .. } => {
            check.enforce()?;
        }
        VersionCheck::RepoOlder { .. } => {}
    }

    let old = match SocleLock::load(root) {
        Ok(lock) => lock,
        Err(err) if args.force => {
            tracing::warn!("ignoring the unreadable socle registry: {err}");
            SocleLock::default()
        }
        Err(err) => return Err(err),
    };
    let edited = socle::edited_by_hand(root, &old)?;
    if !edited.is_empty() {
        if !args.force {
            return Err(Error::blocked(
                format!(
                    "socle files edited by hand since the socle wrote them:{}",
                    list(&edited)
                ),
                "move your changes out of these files, then rerun `modpack update --force` to overwrite them",
            ));
        }
        tracing::warn!("overwriting socle files edited by hand:{}", list(&edited));
    }

    let new = socle::write_socle(root)?;
    let removed = socle::remove_stale(root, &old, &new)?;
    new.save(root)?;
    let from = std::mem::replace(&mut config.version, cli.to_string());
    config.save(root)?;

    println!("Updated the socle from {from} to {cli}.");
    for path in removed {
        println!("Removed {path}, no longer part of the socle.");
    }
    Ok(())
}

fn list(paths: &[String]) -> String {
    paths.iter().map(|p| format!("\n  - {p}")).collect()
}
