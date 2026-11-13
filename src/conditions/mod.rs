//! Checks of the conditions a variant declares in `variants.toml`, run by
//! `modpack test`.
//!
//! Each check gives a [`Status`]: passed, failed with the reason, or skipped
//! with the reason. A check never returns an error: whatever goes wrong while
//! checking (a missing jar, a download that fails) is the reason of a failure,
//! so that one condition never hides the others. What the machine lacks to
//! run a check at all (no Java for `server-starts`) skips it, with the reason.

pub mod server;
mod statics;

use std::path::Path;

use crate::model::manifest::Manifest;
use crate::model::variants::{Condition, Resolved, Side};

/// Names of the conditions `variants.toml` knows, as `--condition` takes them.
pub const NAMES: [&str; 4] = ["side", "deps-complete", "server-starts", "client-starts"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed(String),
    Skipped(String),
}

impl Status {
    pub fn name(&self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed(_) => "failed",
            Status::Skipped(_) => "skipped",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Status::Passed => None,
            Status::Failed(reason) | Status::Skipped(reason) => Some(reason),
        }
    }
}

/// What the checks read: the repository, its manifest, and the machine for
/// the checks that start Minecraft.
pub struct Checker<'a> {
    pub root: &'a Path,
    pub manifest: &'a Manifest,
    pub machine: &'a server::Machine<'a>,
}

impl Checker<'_> {
    /// Checks one condition of a resolved variant.
    pub fn check(&self, condition: &Condition, variant: &Resolved) -> Status {
        match condition {
            Condition::Side(side) => statics::side(*side, self.manifest, variant),
            Condition::DepsComplete => statics::deps_complete(self.manifest, variant),
            Condition::ServerStarts { timeout } => {
                server::server_starts(self.root, self.manifest, variant, *timeout, self.machine)
            }
            Condition::ClientStarts => Status::Skipped(
                "client-starts is not implemented: starting the client without a real display is not settled yet"
                    .to_owned(),
            ),
        }
    }
}

/// The condition as written in `variants.toml`, for reports.
pub fn label(condition: &Condition) -> String {
    match condition {
        Condition::Side(Side::Client) => "side = \"client\"".to_owned(),
        Condition::Side(Side::Server) => "side = \"server\"".to_owned(),
        Condition::ServerStarts { timeout } => {
            format!("server-starts (timeout {}s)", timeout.as_secs())
        }
        other => other.name().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn labels_read_like_variants_toml() {
        assert_eq!(label(&Condition::Side(Side::Server)), "side = \"server\"");
        assert_eq!(label(&Condition::DepsComplete), "deps-complete");
        assert_eq!(
            label(&Condition::ServerStarts {
                timeout: Duration::from_secs(180)
            }),
            "server-starts (timeout 180s)"
        );
        for c in [
            Condition::Side(Side::Client),
            Condition::DepsComplete,
            Condition::ClientStarts,
        ] {
            assert!(NAMES.contains(&c.name()));
        }
    }

    #[test]
    fn client_starts_is_skipped_with_the_reason() {
        let manifest = statics::tests::manifest();
        let machine = server::tests::machine_without_network();
        let checker = Checker {
            root: Path::new("."),
            manifest: &manifest,
            machine: &machine,
        };
        let variant = Resolved {
            name: "default".into(),
            keys: Vec::new(),
            conditions: Vec::new(),
        };
        let status = checker.check(&Condition::ClientStarts, &variant);
        assert_eq!(status.name(), "skipped");
        assert!(status.reason().unwrap().contains("not implemented"));
    }
}
