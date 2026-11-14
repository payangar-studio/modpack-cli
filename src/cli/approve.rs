//! `modpack approve`: approves a local mod or resource pack for publication,
//! or checks that every local entry is approved (the gate of the publish
//! workflow).

use crate::context::Context;
use crate::error::Result;
use crate::ops::local::{approve, check_approved};

#[derive(Debug, clap::Args)]
#[command(
    after_help = "A local entry is a file of mods-local/ or resourcepacks-local/, tracked by git. Its approval is the sha512 of the file, recorded in modpack.lock.toml: a changed file loses it. The publish workflow refuses to publish while a local entry is not approved."
)]
pub struct Args {
    /// Key of the local entry to approve, after reviewing its file
    #[arg(required_unless_present = "check", conflicts_with = "check")]
    pub key: Option<String>,
    /// Fail (exit code 3) while a local entry is not approved, naming each one
    #[arg(long)]
    pub check: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let out = &mut std::io::stdout().lock();
    match args.key {
        Some(key) => approve(root, &key, out),
        None => check_approved(root, out),
    }
}
