//! `modpack auth`: stores the platform secrets in the system keyring.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("auth"))
}
