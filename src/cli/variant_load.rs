//! `modpack variant load`: loads a variant into the instance.

use std::io::Write;

use crate::context::Context;
use crate::error::Result;
use crate::launcher::running::Processes;
use crate::model::instance::Instances;
use crate::platform::Registry;
use crate::store::download::{Http, Sources};
use crate::variant::{self, LoadContext};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Variant to load
    pub variant: String,
    /// Load even if the instance drifted from the last loaded variant
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let instances_file = Instances::default_path()?;
    let registry = Registry::builtin();
    let http = Http::new();
    let load_ctx = LoadContext {
        root,
        instances_file: &instances_file,
        game: &Processes,
        sources: Sources {
            registry: &registry,
            fetch: &http,
        },
    };
    let report = variant::load(&load_ctx, &args.variant, args.force)?;
    write!(std::io::stdout().lock(), "{report}")?;
    Ok(())
}
