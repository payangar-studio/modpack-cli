//! `modpack update`: brings the repository's socle to this CLI's version.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Overwrite socle files edited by hand
    #[arg(long)]
    pub force: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("update"))
}
