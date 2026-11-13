//! `modpack manifest`: regenerates the manifest from mods/.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Fail if the manifest is out of date instead of rewriting it
    #[arg(long)]
    pub check: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("manifest"))
}
