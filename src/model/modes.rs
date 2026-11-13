//! `modes.toml`: the modes of the pack and their conditions.
//!
//! One level of modes. The default mode holds every mod of the manifest but
//! those it removes; every other mode starts from the default selection,
//! removes and adds keys. Conditions set at the root apply to every mode,
//! the default one included.
//!
//! ```toml
//! [conditions]                  # global
//! deps-complete = true
//!
//! [default]
//! description = "Full pack"
//! remove = ["hardcore-tweaks"]
//! conditions = { side = "client" }
//!
//! [modes.server]
//! remove = ["sodium"]
//! conditions = { side = "server", server-starts = { timeout = "180s" } }
//! ```
//!
//! This module only reads the file; editing it while keeping comments is done
//! with `toml_edit` by the commands that change it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::model::manifest::Manifest;

/// File name of the modes, at the root of the repository.
pub const FILE_NAME: &str = "modes.toml";
/// Name of the default mode on the command line and in reports.
pub const DEFAULT_MODE: &str = "default";
/// Timeout of `server-starts` when the file gives none.
pub const DEFAULT_SERVER_TIMEOUT: Duration = Duration::from_secs(300);

const HINT: &str = "fix modes.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Modes {
    /// Conditions of every mode.
    pub conditions: Vec<Condition>,
    pub default: DefaultMode,
    /// Modes other than the default one, by name.
    pub modes: BTreeMap<String, Mode>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefaultMode {
    pub description: Option<String>,
    pub remove: Vec<String>,
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mode {
    pub description: Option<String>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// Every mod of the mode accepts this side, after its `env`.
    Side(Side),
    /// The required dependencies of every mod are in the mode.
    DepsComplete,
    /// A dedicated server with the mode's mods reaches `Done` within the timeout.
    ServerStarts { timeout: Duration },
    /// The client with the mode's mods reaches the main menu.
    ClientStarts,
}

impl Condition {
    /// Name of the condition in `modes.toml`.
    pub fn name(&self) -> &'static str {
        match self {
            Condition::Side(_) => "side",
            Condition::DepsComplete => "deps-complete",
            Condition::ServerStarts { .. } => "server-starts",
            Condition::ClientStarts => "client-starts",
        }
    }
}

/// A mode once resolved: its mods and its conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub name: String,
    /// Keys of the mode's mods, in manifest key order.
    pub keys: Vec<String>,
    /// Global conditions, then the mode's own.
    pub conditions: Vec<Condition>,
}

/// An inconsistency between `modes.toml` and the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A mode names a key the manifest does not have.
    UnknownKey { mode: String, key: String },
    /// A mode adds a key the default selection already holds.
    AlreadyPresent { mode: String, key: String },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::UnknownKey { mode, key } => {
                write!(
                    f,
                    "mode `{mode}` names `{key}`, which is not in mods.lock.toml"
                )
            }
            Problem::AlreadyPresent { mode, key } => write!(
                f,
                "mode `{mode}` adds `{key}`, which the default mode already holds; remove it from the default mode first"
            ),
        }
    }
}

impl Modes {
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawModes = super::parse_toml(text, FILE_NAME, HINT)?;
        let conditions = parse_conditions(&raw.conditions, "the root")?;

        if !raw.default.add.is_empty() {
            return Err(invalid(
                "[default] cannot `add`: the default mode already holds every mod it does not remove"
                    .to_owned(),
            ));
        }
        if let Some(field) = raw.default.extra.keys().next() {
            return Err(invalid(format!(
                "[default] has an unknown field `{field}` (the default mode only takes `description`, `remove` and `conditions`)"
            )));
        }
        let default = DefaultMode {
            description: raw.default.description,
            remove: raw.default.remove,
            conditions: parse_conditions(&raw.default.conditions, "[default]")?,
        };
        check_no_overlap(&conditions, &default.conditions, "[default]")?;

        let mut modes = BTreeMap::new();
        for (name, raw) in raw.modes {
            let place = format!("[modes.{name}]");
            if name == DEFAULT_MODE {
                return Err(invalid(format!(
                    "{place} is reserved: the default mode is set in [default]"
                )));
            }
            if let Some((field, value)) = raw.extra.iter().next() {
                return Err(invalid(if value.is_table() {
                    format!(
                        "[modes.{name}.{field}] nests a mode in a mode; modes have one level, each one defined from the default mode"
                    )
                } else {
                    format!("{place} has an unknown field `{field}`")
                }));
            }
            let mode = Mode {
                description: raw.description,
                add: raw.add,
                remove: raw.remove,
                conditions: parse_conditions(&raw.conditions, &place)?,
            };
            check_no_overlap(&conditions, &mode.conditions, &place)?;
            modes.insert(name, mode);
        }

        Ok(Self {
            conditions,
            default,
            modes,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&super::read_file(path)?)
    }

    /// Names of every mode, the default one first.
    pub fn names(&self) -> Vec<&str> {
        std::iter::once(DEFAULT_MODE)
            .chain(self.modes.keys().map(String::as_str))
            .collect()
    }

    /// Every inconsistency with the manifest; empty when the modes are sound.
    pub fn validate(&self, manifest: &Manifest) -> Vec<Problem> {
        let known = manifest.keys();
        let mut problems = Vec::new();
        let mut unknown = |mode: &str, keys: &[String]| {
            for key in keys.iter().filter(|k| !known.contains(k.as_str())) {
                problems.push(Problem::UnknownKey {
                    mode: mode.to_owned(),
                    key: key.clone(),
                });
            }
        };
        unknown(DEFAULT_MODE, &self.default.remove);
        for (name, mode) in &self.modes {
            unknown(name, &mode.remove);
            unknown(name, &mode.add);
        }
        let default_selection = self.default_selection(manifest);
        for (name, mode) in &self.modes {
            for key in mode
                .add
                .iter()
                .filter(|k| default_selection.contains(k.as_str()))
            {
                problems.push(Problem::AlreadyPresent {
                    mode: name.clone(),
                    key: key.clone(),
                });
            }
        }
        problems
    }

    /// The mods and conditions of a mode. Fails on an unknown mode, or when
    /// the modes are inconsistent with the manifest.
    pub fn resolve(&self, mode: &str, manifest: &Manifest) -> Result<Resolved> {
        let problems = self.validate(manifest);
        if !problems.is_empty() {
            let list: Vec<String> = problems.iter().map(|p| format!("\n  - {p}")).collect();
            return Err(invalid(format!(
                "modes.toml does not match mods.lock.toml:{}",
                list.concat()
            )));
        }
        let mut selection = self.default_selection(manifest);
        let own_conditions = if mode == DEFAULT_MODE {
            &self.default.conditions
        } else {
            let def = self.modes.get(mode).ok_or_else(|| {
                Error::usage(
                    format!("unknown mode `{mode}`"),
                    format!("use one of: {}", self.names().join(", ")),
                )
            })?;
            for key in &def.remove {
                selection.remove(key.as_str());
            }
            selection.extend(def.add.iter().map(String::as_str));
            &def.conditions
        };
        Ok(Resolved {
            name: mode.to_owned(),
            keys: selection.into_iter().map(str::to_owned).collect(),
            conditions: self
                .conditions
                .iter()
                .chain(own_conditions)
                .cloned()
                .collect(),
        })
    }

    fn default_selection<'m>(&self, manifest: &'m Manifest) -> BTreeSet<&'m str> {
        let mut keys = manifest.keys();
        for key in &self.default.remove {
            keys.remove(key.as_str());
        }
        keys
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModes {
    #[serde(default)]
    conditions: toml::Table,
    #[serde(default)]
    default: RawMode,
    #[serde(default)]
    modes: BTreeMap<String, RawMode>,
}

/// A mode as written. `extra` catches what serde would otherwise reject
/// blindly, to explain it: a nested mode, a misspelt field.
#[derive(Deserialize, Default)]
struct RawMode {
    description: Option<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    conditions: toml::Table,
    #[serde(flatten)]
    extra: toml::Table,
}

fn invalid(what: String) -> Error {
    Error::usage(format!("{FILE_NAME}: {what}"), HINT)
}

fn parse_conditions(table: &toml::Table, place: &str) -> Result<Vec<Condition>> {
    let mut conditions = Vec::new();
    for (name, value) in table {
        let bad = || {
            invalid(format!(
                "condition `{name}` in {place} has an invalid value `{value}`"
            ))
        };
        let condition = match name.as_str() {
            "side" => match value.as_str() {
                Some("client") => Some(Condition::Side(Side::Client)),
                Some("server") => Some(Condition::Side(Side::Server)),
                _ => return Err(bad()),
            },
            "deps-complete" => value
                .as_bool()
                .ok_or_else(bad)?
                .then_some(Condition::DepsComplete),
            "client-starts" => value
                .as_bool()
                .ok_or_else(bad)?
                .then_some(Condition::ClientStarts),
            "server-starts" => match value {
                toml::Value::Boolean(on) => on.then_some(Condition::ServerStarts {
                    timeout: DEFAULT_SERVER_TIMEOUT,
                }),
                toml::Value::Table(t) if t.keys().all(|k| k == "timeout") => {
                    let timeout = match t.get("timeout") {
                        None => DEFAULT_SERVER_TIMEOUT,
                        Some(v) => v.as_str().and_then(parse_duration).ok_or_else(bad)?,
                    };
                    Some(Condition::ServerStarts { timeout })
                }
                _ => return Err(bad()),
            },
            _ => {
                return Err(invalid(format!(
                    "unknown condition `{name}` in {place} (known: side, deps-complete, server-starts, client-starts)"
                )));
            }
        };
        conditions.extend(condition);
    }
    Ok(conditions)
}

/// A global condition repeated in a mode would leave two values for one test.
fn check_no_overlap(global: &[Condition], own: &[Condition], place: &str) -> Result<()> {
    match own
        .iter()
        .find(|c| global.iter().any(|g| g.name() == c.name()))
    {
        Some(c) => Err(invalid(format!(
            "condition `{}` is set both at the root, for every mode, and in {place}",
            c.name()
        ))),
        None => Ok(()),
    }
}

/// `90s`, `3m`, `1h`.
fn parse_duration(text: &str) -> Option<Duration> {
    let (number, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = number.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n.checked_mul(60)?,
        "h" => n.checked_mul(3600)?,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
    }

    fn manifest() -> Manifest {
        Manifest::load(&fixtures().join(crate::model::manifest::FILE_NAME)).unwrap()
    }

    fn modes() -> Modes {
        Modes::load(&fixtures().join(FILE_NAME)).unwrap()
    }

    /// Every key of the fixture manifest but `except`.
    fn all_but(except: &[&str]) -> Vec<String> {
        manifest()
            .keys()
            .into_iter()
            .filter(|k| !except.contains(k))
            .map(str::to_owned)
            .collect()
    }

    fn parse_err(text: &str) -> String {
        let err = Modes::parse(text).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        err.to_string()
    }

    #[test]
    fn fixture_is_consistent_with_the_manifest() {
        assert_eq!(modes().validate(&manifest()), Vec::new());
        assert_eq!(modes().names(), ["default", "hardcore", "server"]);
    }

    #[test]
    fn default_mode_is_the_manifest_minus_its_removals() {
        let resolved = modes().resolve(DEFAULT_MODE, &manifest()).unwrap();
        assert_eq!(resolved.keys, all_but(&["hardcore-tweaks"]));
        assert_eq!(
            resolved.conditions,
            [Condition::DepsComplete, Condition::Side(Side::Client)]
        );
    }

    #[test]
    fn server_mode_removes_from_the_default_selection() {
        let resolved = modes().resolve("server", &manifest()).unwrap();
        assert_eq!(
            resolved.keys,
            all_but(&["hardcore-tweaks", "sodium", "iris", "jei"])
        );
        assert_eq!(
            resolved.conditions,
            [
                Condition::DepsComplete,
                Condition::ServerStarts {
                    timeout: Duration::from_secs(180)
                },
                Condition::Side(Side::Server),
            ]
        );
    }

    #[test]
    fn mode_adds_a_mod_removed_from_the_default() {
        let resolved = modes().resolve("hardcore", &manifest()).unwrap();
        assert_eq!(resolved.keys, all_but(&[]));
    }

    #[test]
    fn global_conditions_are_in_every_mode() {
        let modes = modes();
        for name in modes.names() {
            let resolved = modes.resolve(name, &manifest()).unwrap();
            assert_eq!(resolved.conditions.first(), Some(&Condition::DepsComplete));
        }
    }

    #[test]
    fn unknown_mode_is_a_usage_error_listing_the_modes() {
        let err = modes().resolve("lowend", &manifest()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("unknown mode `lowend`"));
        assert!(err.hint().contains("default, hardcore, server"));
    }

    #[test]
    fn unknown_key_is_reported() {
        let modes =
            Modes::parse("[default]\nremove = [\"nope\"]\n[modes.a]\nadd = [\"ghost\"]\n").unwrap();
        assert_eq!(
            modes.validate(&manifest()),
            [
                Problem::UnknownKey {
                    mode: "default".into(),
                    key: "nope".into()
                },
                Problem::UnknownKey {
                    mode: "a".into(),
                    key: "ghost".into()
                },
            ]
        );
        let err = modes.resolve("a", &manifest()).unwrap_err();
        assert!(err.to_string().contains("`ghost`"));
    }

    #[test]
    fn adding_a_key_already_in_the_default_is_reported() {
        let modes = Modes::parse("[modes.a]\nadd = [\"jei\"]\n").unwrap();
        assert_eq!(
            modes.validate(&manifest()),
            [Problem::AlreadyPresent {
                mode: "a".into(),
                key: "jei".into()
            }]
        );
    }

    #[test]
    fn unknown_condition_is_refused() {
        let err = parse_err("[modes.a.conditions]\nfast = true\n");
        assert!(err.contains("unknown condition `fast` in [modes.a]"));
        assert!(parse_err("[conditions]\nfast = true\n").contains("in the root"));
    }

    #[test]
    fn second_level_of_mode_is_refused() {
        let err = parse_err("[modes.server.lowend]\nremove = [\"jei\"]\n");
        assert!(err.contains("[modes.server.lowend] nests a mode in a mode"));
    }

    #[test]
    fn default_mode_cannot_add_nor_be_redefined() {
        assert!(parse_err("[default]\nadd = [\"jei\"]\n").contains("[default] cannot `add`"));
        assert!(parse_err("[default]\nremov = []\n").contains("unknown field `remov`"));
        assert!(parse_err("[modes.default]\n").contains("reserved"));
    }

    #[test]
    fn a_global_condition_cannot_be_repeated_in_a_mode() {
        let err =
            parse_err("[conditions]\nside = \"client\"\n[modes.a.conditions]\nside = \"server\"\n");
        assert!(err.contains("`side` is set both at the root"));
    }

    #[test]
    fn condition_values_are_checked() {
        assert!(parse_err("[conditions]\nside = \"both\"\n").contains("invalid value"));
        assert!(
            parse_err("[conditions]\nserver-starts = { timeout = \"soon\" }\n")
                .contains("invalid value")
        );
        assert!(
            parse_err("[conditions]\nserver-starts = { wait = \"1m\" }\n")
                .contains("invalid value")
        );
        let modes = Modes::parse(
            "[conditions]\nserver-starts = true\nclient-starts = true\ndeps-complete = false\n",
        )
        .unwrap();
        assert_eq!(
            modes.conditions,
            [
                Condition::ClientStarts,
                Condition::ServerStarts {
                    timeout: DEFAULT_SERVER_TIMEOUT
                },
            ]
        );
    }

    #[test]
    fn durations_take_seconds_minutes_hours() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("3m"), Some(Duration::from_secs(180)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("3"), None);
        assert_eq!(parse_duration("s"), None);
        assert_eq!(parse_duration("3d"), None);
    }

    #[test]
    fn empty_file_is_a_default_mode_with_every_mod() {
        let resolved = Modes::parse("")
            .unwrap()
            .resolve(DEFAULT_MODE, &manifest())
            .unwrap();
        assert_eq!(resolved.keys, all_but(&[]));
        assert!(resolved.conditions.is_empty());
    }
}
