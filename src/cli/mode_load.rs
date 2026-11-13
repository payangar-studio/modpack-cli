//! `modpack mode load`: loads a mode into the instance.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Mode to load
    pub mode: String,
    /// Load even if the instance drifted from the last loaded mode
    #[arg(long)]
    pub force: bool,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("mode load"))
}
