//! `modpack approve`: approves a local mod for publication, or checks that
//! every local mod is approved (the gate of the publish workflow).

use crate::context::Context;
use crate::error::Result;
use crate::ops::local::{approve, check_approved};

#[derive(Debug, clap::Args)]
#[command(
    after_help = "A local mod is a jar of mods-local/, tracked by git. Its approval is the sha512 of the jar, recorded in mods.lock.toml: a changed jar loses it. The publish workflow refuses to publish while a local mod is not approved."
)]
pub struct Args {
    /// Key of the local mod to approve, after reviewing its jar
    #[arg(required_unless_present = "check", conflicts_with = "check")]
    pub key: Option<String>,
    /// Fail (exit code 3) while a local mod is not approved, naming each one
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
