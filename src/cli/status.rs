//! `modpack status`: socle version, the pack's instance, loaded variant, drift,
//! unknown jars, and how mods/ compares to the manifest. Writes nothing.

use std::io::Write;

use crate::context::Context;
use crate::error::Result;
use crate::model::instance::Instances;
use crate::status::status;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Print the report as JSON, in the format documented in docs/status.md
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root_unchecked()?;
    let report = status(root, &Instances::default_path()?)?;
    let mut out = std::io::stdout().lock();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &report)?;
        writeln!(out)?;
    } else {
        report.write_text(&mut out)?;
    }
    Ok(())
}
