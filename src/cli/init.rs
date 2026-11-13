//! `modpack init`: creates a modpack repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::context::{Context, SOCLE_DIR};
use crate::error::{Context as _, Error, Result};
use crate::model::socle::InitValues;

/// Loaders, as Modrinth names them.
const LOADERS: &[&str] = &["neoforge", "fabric", "forge", "quilt"];

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
    let dir = args.dir;
    if dir.join(SOCLE_DIR).exists() {
        return Err(Error::usage(
            format!("{} already holds a modpack socle", dir.display()),
            format!(
                "run `modpack update` in {} to bring its socle up to date, or pick another directory",
                dir.display()
            ),
        ));
    }

    let p = &ctx.prompt;
    let name = p.text(args.name, "--name", "Modpack name")?;
    let minecraft = p.text(args.mc_version, "--mc-version", "Minecraft version")?;
    let loader = p.select(args.loader, "--loader", "Mod loader", &loaders())?;
    if !LOADERS.contains(&loader.as_str()) {
        return Err(Error::usage(
            format!("unknown loader `{loader}`"),
            format!("pass --loader with one of: {}", LOADERS.join(", ")),
        ));
    }
    let loader_version = p.text(args.loader_version, "--loader-version", "Loader version")?;
    let author = p.text(args.author, "--author", "Author")?;

    std::fs::create_dir_all(&dir).context(format!("creating {}", dir.display()))?;
    git_init(&dir)?;
    crate::socle::init(
        &dir,
        InitValues {
            name: name.clone(),
            minecraft,
            loader,
            loader_version,
            author,
        },
    )?;

    println!("Created modpack `{name}` in {}.", dir.display());
    println!("Review the files, commit them, then run `modpack setup` to prepare this machine.");
    Ok(())
}

fn loaders() -> Vec<String> {
    LOADERS.iter().map(|l| l.to_string()).collect()
}

/// Makes `dir` a git repository unless it already is one. Never commits.
fn git_init(dir: &Path) -> Result<()> {
    if dir.join(".git").exists() {
        return Ok(());
    }
    let hint = format!(
        "install git, or run `git init` in {} yourself, then rerun `modpack init`",
        dir.display()
    );
    let output = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(dir)
        .output()
        .map_err(|e| Error::internal("could not run `git init`", hint.clone()).with_source(e))?;
    if !output.status.success() {
        return Err(Error::internal(
            format!(
                "`git init` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            hint,
        ));
    }
    Ok(())
}
