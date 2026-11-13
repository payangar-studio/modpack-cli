//! Everything that depends on the Modrinth App, so that the rest of the CLI
//! knows nothing about it.
//!
//! - [`dirs`]: where the app keeps its instances (`profiles/`).
//! - [`register`]: registering a pack's instance in the app, by generating a
//!   `.mrpack`, opening it with the app and waiting for the instance directory
//!   the user's click creates.
//! - [`running`]: whether Minecraft runs from a given instance.
//!
//! The CLI never writes into the app's database (`app.db`) and never clicks
//! in its place: the user confirms the import in the app.

pub mod dirs;
pub mod register;
pub mod running;

/// Name of the platform whose pack artifact (`.mrpack`) the app imports.
pub const PACK_PLATFORM: &str = "modrinth";
