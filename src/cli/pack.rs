//! `modpack pack`: builds the .mrpack and the CurseForge zip.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Mode to pack; all published modes when omitted
    pub mode: Option<String>,
    /// Build for this platform only (modrinth or curseforge)
    #[arg(long)]
    pub platform: Option<String>,
    /// Output directory
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("pack"))
}
