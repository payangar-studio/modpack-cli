//! `modpack remove`: removes a mod from the repository and its modes.

use crate::context::Context;
use crate::error::Result;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Manifest key of the mod
    pub key: String,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    crate::ops::mods::remove(root, &args.key, &mut std::io::stdout().lock())
}
