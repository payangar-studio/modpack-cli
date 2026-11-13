use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;
use modpack_cli::cli::{self, Cli};
use modpack_cli::context::Context;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let verbosity = cli.verbosity.level();
    init_logs(verbosity);

    match Context::detect(verbosity).and_then(|ctx| cli::run(cli.command, &ctx)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            err.exit_code()
        }
    }
}

/// Logs go to stderr. `-v`/`-q` set the level; without them, `RUST_LOG` does,
/// and the default is `warn`.
fn init_logs(verbosity: i8) {
    let filter = match verbosity {
        0 => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        ..=-2 => EnvFilter::new("off"),
        -1 => EnvFilter::new("error"),
        1 => EnvFilter::new("info"),
        2 => EnvFilter::new("debug"),
        3.. => EnvFilter::new("trace"),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(false)
        .without_time()
        .init();
}
