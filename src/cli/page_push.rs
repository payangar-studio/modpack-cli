//! `modpack page push`: pushes the versioned page to the platforms that have an API.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Push to this platform only
    #[arg(long)]
    pub platform: Option<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("page push"))
}
