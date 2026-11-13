//! Library behind the `modpack` binary.
//!
//! This file only registers modules. New modules are added here, one line
//! each; merge conflicts on these lines are resolved by keeping both.

pub mod cli;
pub mod context;
pub mod error;
pub mod prompt;
