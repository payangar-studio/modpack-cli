# modpack-cli

Rust CLI of the Minecraft modpack socle: one crate, `modpack-cli`, building one binary, `modpack`.
The design lives in `payangar-studio/environment`, `docs/design/00-cadrage.md` and `docs/design/01-socle-modpack.md`; it is the source of truth for behaviour.

## Layout

- `src/main.rs`: entry point. Parses the command line, sets up logs, builds the context, calls `cli::run`, maps errors to exit codes.
- `src/lib.rs`: module registry, one `pub mod` line per module.
- `src/cli/mod.rs`: the clap command tree and the dispatch to handlers.
- `src/cli/<command>.rs`: one file per command (`mode load` is `mode_load.rs`), holding its `Args` and its `run(args, ctx)` handler.
- `src/prompt.rs`: "ask, or read the argument". Every value a command needs goes through it, so that every command can run without a terminal.
- `src/error.rs`: error model. Every error says what happened and what to do. Exit codes: 0 success, 1 internal error, 2 usage error (missing argument), 3 blocked (drift or file edited by hand, overridable with `--force`).
- `src/context.rs`: context passed to handlers (repository root found by walking up to `.modpack/`, prompt, verbosity).
- `tests/`: integration tests of the binary; `tests/fixtures/`: their fixtures.

## Who edits what

Each issue owns its command handler (`src/cli/<command>.rs`, including its `Args`) and the modules it creates.

- `src/main.rs`, `src/cli/mod.rs`, `src/prompt.rs`, `src/error.rs` and `src/context.rs` only change through an issue dedicated to them.
- `src/lib.rs` and `Cargo.toml` are append-only: add a module line or a dependency, never change the others. Merge conflicts on them are resolved by keeping both sides.

## Conventions

- Logs go to stderr through `tracing`; command output goes to stdout.
- Machine locations (caches, data, config) come from the `directories` crate, never from hard-coded Linux paths, so that Windows follows without a rewrite.
- User-facing messages are in English.

## Tests

- Unit tests live in each module (`#[cfg(test)] mod tests`).
- Integration tests of the CLI live in `tests/` and drive the binary with `assert_cmd`. Fixtures go in `tests/fixtures/`.
- No network call in default tests. A test that needs the network is `#[ignore]` and run explicitly (`cargo test -- --ignored`); a CurseForge one also skips itself when `CURSEFORGE_API_KEY` is not set.
- Locally, run only the tests of the code you touched (`cargo test <filter>`), plus `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`. The full suite runs in CI on every push and pull request.

## Release

Pushing a `v*` tag builds the static Linux binary (`x86_64-unknown-linux-musl`) and publishes a GitHub Release with `modpack-<version>-x86_64-unknown-linux-musl.tar.gz` and its `.sha256`. A tag with a suffix (`v0.1.0-rc1`) is published as a prerelease.
