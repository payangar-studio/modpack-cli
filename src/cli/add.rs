//! `modpack add`: adds a mod to the repository.

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Modrinth or CurseForge project, or path to a local jar
    #[arg(value_name = "MOD")]
    pub mod_ref: String,
    /// Version of the mod to add
    #[arg(long)]
    pub version: Option<String>,
    /// Add the mod to this mode only
    #[arg(long)]
    pub mode: Option<String>,
}

pub fn run(_args: Args, _ctx: &Context) -> Result<()> {
    Err(Error::not_implemented("add"))
}
