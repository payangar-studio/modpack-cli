//! Command tree of the `modpack` binary.
//!
//! Each command lives in `src/cli/<command>.rs`, which owns its `Args` and
//! its `run(args, ctx)` handler.

use clap::{Parser, Subcommand};

use crate::context::Context;
use crate::error::Result;

pub mod add;
pub mod auth;
pub mod init;
pub mod install;
pub mod manifest;
pub mod mode_list;
pub mod mode_load;
pub mod pack;
pub mod page_push;
pub mod remove;
pub mod setup;
pub mod status;
pub mod test;
pub mod update;

/// CLI of the Minecraft modpack socle.
#[derive(Debug, Parser)]
#[command(name = "modpack", version)]
pub struct Cli {
    #[command(flatten)]
    pub verbosity: Verbosity,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Args)]
pub struct Verbosity {
    /// More logs on stderr (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    /// Fewer logs on stderr (-q errors only, -qq none)
    #[arg(short, long, action = clap::ArgAction::Count, global = true, conflicts_with = "verbose")]
    pub quiet: u8,
}

impl Verbosity {
    /// `-v` count minus `-q` count.
    pub fn level(&self) -> i8 {
        self.verbose.min(10) as i8 - self.quiet.min(10) as i8
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a modpack repository: socle, layout, default mode
    Init(init::Args),
    /// Prepare a clone on this machine: hooks, jars, Modrinth instance, default mode
    Setup(setup::Args),
    /// Bring the repository's socle to this CLI's version
    Update(update::Args),
    /// Add a mod from Modrinth, CurseForge or a local jar
    Add(add::Args),
    /// Remove a mod from the repository and from the modes that list it
    Remove(remove::Args),
    /// Regenerate the manifest from mods/
    Manifest(manifest::Args),
    /// Download the missing jars into mods/, then reload the current mode
    Install(install::Args),
    /// List or load modes
    Mode {
        #[command(subcommand)]
        command: ModeCommand,
    },
    /// Loaded mode, drift, jars unknown to the manifest
    Status(status::Args),
    /// Check the conditions of a mode, or of all modes
    Test(test::Args),
    /// Build the .mrpack and the CurseForge zip
    Pack(pack::Args),
    /// Manage the versioned project page
    Page {
        #[command(subcommand)]
        command: PageCommand,
    },
    /// Store the platform secrets in the system keyring
    Auth(auth::Args),
}

#[derive(Debug, Subcommand)]
pub enum ModeCommand {
    /// List the modes and show the loaded one
    List(mode_list::Args),
    /// Load a mode into the instance
    Load(mode_load::Args),
}

#[derive(Debug, Subcommand)]
pub enum PageCommand {
    /// Push the versioned page to the platforms that have an API
    Push(page_push::Args),
}

pub fn run(command: Command, ctx: &Context) -> Result<()> {
    match command {
        Command::Init(args) => init::run(args, ctx),
        Command::Setup(args) => setup::run(args, ctx),
        Command::Update(args) => update::run(args, ctx),
        Command::Add(args) => add::run(args, ctx),
        Command::Remove(args) => remove::run(args, ctx),
        Command::Manifest(args) => manifest::run(args, ctx),
        Command::Install(args) => install::run(args, ctx),
        Command::Mode { command } => match command {
            ModeCommand::List(args) => mode_list::run(args, ctx),
            ModeCommand::Load(args) => mode_load::run(args, ctx),
        },
        Command::Status(args) => status::run(args, ctx),
        Command::Test(args) => test::run(args, ctx),
        Command::Pack(args) => pack::run(args, ctx),
        Command::Page { command } => match command {
            PageCommand::Push(args) => page_push::run(args, ctx),
        },
        Command::Auth(args) => auth::run(args, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }
}
