//! `modpack init`: creates a modpack repository.

use std::path::PathBuf;

use crate::context::Context;
use crate::error::{Error, Result};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Directory to create the repository in
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Name of the modpack
    #[arg(long)]
    pub name: Option<String>,
    /// Minecraft version
    #[arg(long)]
    pub mc_version: Option<String>,
    /// Mod loader
    #[arg(long)]
    pub loader: Option<String>,
    /// Mod loader version
    #[arg(long)]
    pub loader_version: Option<String>,
    /// Author of the modpack
    #[arg(long)]
    pub author: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let p = &ctx.prompt;
    let _name = p.text(args.name, "--name", "Modpack name")?;
    let _mc_version = p.text(args.mc_version, "--mc-version", "Minecraft version")?;
    let _loader = p.text(args.loader, "--loader", "Mod loader")?;
    let _loader_version = p.text(args.loader_version, "--loader-version", "Loader version")?;
    let _author = p.text(args.author, "--author", "Author")?;
    Err(Error::not_implemented("init"))
}
