//! `modpack test`: checks the conditions of a mode, or of all modes.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Mode to check; all modes when omitted
    pub mode: Option<String>,
    /// Check only this condition
    #[arg(long)]
    pub condition: Option<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("test"))
}
