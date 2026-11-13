//! `modpack mode load`: loads a mode into the instance.

use std::io::Write;

use crate::context::Context;
use crate::error::Result;
use crate::launcher::running::Processes;
use crate::mode::{self, LoadContext};
use crate::model::instance::Instances;
use crate::platform::Registry;
use crate::store::download::{Http, Sources};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Mode to load
    pub mode: String,
    /// Load even if the instance drifted from the last loaded mode
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
    let report = mode::load(&load_ctx, &args.mode, args.force)?;
    write!(std::io::stdout().lock(), "{report}")?;
    Ok(())
}
