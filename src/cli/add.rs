//! `modpack add`: adds a mod or a resource pack to the repository.

use crate::context::Context;
use crate::error::Result;
use crate::ops::mods::{AddRequest, add};
use crate::platform::Registry;
use crate::store::download::{Http, Sources};

#[derive(Debug, clap::Args)]
#[command(after_help = "Examples:
  modpack add sodium                                      every variant
  modpack add sodium --variant default --variant lite     the default and lite variants; every other variant removes it
  modpack add sodium --variant default                    the default variant only
  modpack add ./tweaks.jar --variant server               the server variant only
  modpack add faithful-32x                                a resource pack, in every variant
  modpack add ./house.zip                                 a local resource pack")]
pub struct Args {
    /// Modrinth or CurseForge project (slug, id or URL), or path to a local
    /// file: a .jar for a mod, a .zip for a resource pack
    #[arg(value_name = "MOD")]
    pub mod_ref: String,
    /// Version to add
    #[arg(long)]
    pub version: Option<String>,
    /// Add it to this variant only; repeat for several variants. `default` names
    /// the default variant: with it, the variants not named remove it. Without
    /// --variant, it is in every variant
    #[arg(long = "variant", value_name = "VARIANT")]
    pub variants: Vec<String>,
    /// Direct URL of a local file that no platform hosts
    #[arg(long)]
    pub url: Option<String>,
    /// Add the missing required dependencies without asking; on a mod already
    /// in the pack, add only those
    #[arg(long)]
    pub with_deps: bool,
    /// Replace a local entry even when its file is not committed in git,
    /// deleting that file
    #[arg(long)]
    pub force: bool,
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
        variants: args.variants,
        url: args.url,
        with_deps: args.with_deps,
        force: args.force,
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
