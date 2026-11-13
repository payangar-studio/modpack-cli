//! `modpack pack`: builds the .mrpack and the CurseForge zip.

use crate::context::Context;
use crate::error::Result;
use crate::pack::{self, Request};
use crate::platform::Registry;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Mode to pack; all modes when omitted
    pub mode: Option<String>,
    /// Build for this platform only (modrinth or curseforge)
    #[arg(long)]
    pub platform: Option<String>,
    /// Output directory [default: dist/ at the repository root]
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
    /// Version of the pack [default: the git tag on HEAD without its `v`, else 0.0.0-dev]
    #[arg(long)]
    pub pack_version: Option<String>,
    /// Print the artifacts built and the platforms blocked as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let version = pack::version(root, args.pack_version.as_deref())?;
    let out = args.out.unwrap_or_else(|| root.join(pack::OUT_DIR));
    let report = pack::build(
        root,
        &Registry::builtin(),
        &Request {
            mode: args.mode.as_deref(),
            platform: args.platform.as_deref(),
            out_dir: &out,
            version: &version,
        },
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for a in &report.artifacts {
            println!(
                "Built {} ({} pack of mode `{}`).",
                a.path.display(),
                a.platform,
                a.mode
            );
        }
        for warning in &report.warnings {
            println!("warning: {warning}");
        }
    }
    match report.blocked_error() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
