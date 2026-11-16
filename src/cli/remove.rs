//! `modpack remove`: removes a mod or a resource pack from the repository and
//! its variants.

use crate::context::Context;
use crate::error::Result;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Manifest key of the entry
    pub key: String,
    /// Remove a local entry even when its file is not committed in git,
    /// deleting that file
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    crate::ops::mods::remove(root, &args.key, args.force, &mut std::io::stdout().lock())
}
