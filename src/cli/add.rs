//! `modpack add`: adds a mod to the repository.

use crate::context::Context;
use crate::error::Result;
use crate::ops::mods::{AddRequest, add};
use crate::platform::Registry;
use crate::store::download::{Http, Sources};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Modrinth or CurseForge project (slug, id or URL), or path to a local jar
    #[arg(value_name = "MOD")]
    pub mod_ref: String,
    /// Version of the mod to add
    #[arg(long)]
    pub version: Option<String>,
    /// Add the mod to this mode only
    #[arg(long)]
    pub mode: Option<String>,
    /// Direct URL of a local jar that no platform hosts
    #[arg(long)]
    pub url: Option<String>,
    /// Add the missing required dependencies without asking
    #[arg(long)]
    pub with_deps: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let registry = Registry::builtin();
    let http = Http::new();
    let sources = Sources {
        registry: &registry,
        fetch: &http,
    };
    let req = AddRequest {
        mod_ref: args.mod_ref,
        version: args.version,
        mode: args.mode,
        url: args.url,
        with_deps: args.with_deps,
    };
    add(
        root,
        sources,
        &ctx.prompt,
        &req,
        &mut std::io::stdout().lock(),
    )?;
    Ok(())
}
