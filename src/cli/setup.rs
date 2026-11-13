//! `modpack setup`: prepares a clone on this machine.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Do not launch the Modrinth app
    #[arg(long)]
    pub no_launch: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("setup"))
}
