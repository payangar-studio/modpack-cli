//! Library behind the `modpack` binary.
//!
//! This file only registers modules. New modules are added here, one line
//! each; merge conflicts on these lines are resolved by keeping both.

pub mod cli;
pub mod conditions;
pub mod context;
pub mod error;
pub mod launcher;
pub mod model;
pub mod platform;
pub mod prompt;
pub mod secrets;
pub mod socle;
pub mod status;
pub mod store;
