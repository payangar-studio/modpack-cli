//! `modpack install`: downloads the missing jars into mods/.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("install"))
}
