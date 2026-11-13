//! `modpack remove`: removes a mod from the repository and its modes.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Manifest key of the mod
    pub key: String,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("remove"))
}
