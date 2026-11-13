//! `modpack remove`: removes a mod from the repository and its variants.

use crate::context::Context;
use crate::error::Result;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Manifest key of the mod
    pub key: String,
    /// Remove a local mod even when its jar is not committed in git,
    /// deleting that jar
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    crate::ops::mods::remove(root, &args.key, args.force, &mut std::io::stdout().lock())
}
