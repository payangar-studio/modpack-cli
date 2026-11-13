//! `variants.toml`: the variants of the pack and their conditions.
//!
//! One level of variants. The default variant holds every mod of the manifest but
//! those it removes; every other variant starts from the default selection,
//! removes and adds keys. Conditions set at the root apply to every variant,
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
//! [variants.server]
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

/// File name of the variants, at the root of the repository.
pub const FILE_NAME: &str = "variants.toml";
/// Name of the default variant on the command line and in reports.
pub const DEFAULT_VARIANT: &str = "default";
/// Timeout of `server-starts` when the file gives none.
pub const DEFAULT_SERVER_TIMEOUT: Duration = Duration::from_secs(300);

const HINT: &str = "fix variants.toml";

#[derive(Debug, Clone, PartialEq)]
pub struct Variants {
    /// Conditions of every variant.
    pub conditions: Vec<Condition>,
    pub default: DefaultVariant,
    /// Variants other than the default one, by name.
    pub variants: BTreeMap<String, Variant>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefaultVariant {
    pub description: Option<String>,
    pub remove: Vec<String>,
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Variant {
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
    /// Every mod of the variant accepts this side, after its `env`.
    Side(Side),
    /// The required dependencies of every mod are in the variant.
    DepsComplete,
    /// A dedicated server with the variant's mods reaches `Done` within the timeout.
    ServerStarts { timeout: Duration },
    /// The client with the variant's mods reaches the main menu.
    ClientStarts,
}

impl Condition {
    /// Name of the condition in `variants.toml`.
    pub fn name(&self) -> &'static str {
        match self {
            Condition::Side(_) => "side",
            Condition::DepsComplete => "deps-complete",
            Condition::ServerStarts { .. } => "server-starts",
            Condition::ClientStarts => "client-starts",
        }
    }
}

/// A variant once resolved: its mods and its conditions.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub name: String,
    /// Keys of the variant's mods, in manifest key order.
    pub keys: Vec<String>,
    /// Global conditions, then the variant's own.
    pub conditions: Vec<Condition>,
}

/// An inconsistency between `variants.toml` and the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A variant names a key the manifest does not have.
    UnknownKey { variant: String, key: String },
    /// A variant adds a key the default selection already holds.
    AlreadyPresent { variant: String, key: String },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::UnknownKey { variant, key } => {
                write!(
                    f,
                    "variant `{variant}` names `{key}`, which is not in mods.lock.toml"
                )
            }
            Problem::AlreadyPresent { variant, key } => write!(
                f,
                "variant `{variant}` adds `{key}`, which the default variant already holds; remove it from the default variant first"
            ),
        }
    }
}

impl Variants {
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawVariants = super::parse_toml(text, FILE_NAME, HINT)?;
        let conditions = parse_conditions(&raw.conditions, "the root")?;

        if !raw.default.add.is_empty() {
            return Err(invalid(
                "[default] cannot `add`: the default variant already holds every mod it does not remove"
                    .to_owned(),
            ));
        }
        if let Some(field) = raw.default.extra.keys().next() {
            return Err(invalid(format!(
                "[default] has an unknown field `{field}` (the default variant only takes `description`, `remove` and `conditions`)"
            )));
        }
        let default = DefaultVariant {
            description: raw.default.description,
            remove: raw.default.remove,
            conditions: parse_conditions(&raw.default.conditions, "[default]")?,
        };
        check_no_overlap(&conditions, &default.conditions, "[default]")?;

        let mut variants = BTreeMap::new();
        for (name, raw) in raw.variants {
            let place = format!("[variants.{name}]");
            if name == DEFAULT_VARIANT {
                return Err(invalid(format!(
                    "{place} is reserved: the default variant is set in [default]"
                )));
            }
            if let Some((field, value)) = raw.extra.iter().next() {
                return Err(invalid(if value.is_table() {
                    format!(
                        "[variants.{name}.{field}] nests a variant in a variant; variants have one level, each one defined from the default variant"
                    )
                } else {
                    format!("{place} has an unknown field `{field}`")
                }));
            }
            let variant = Variant {
                description: raw.description,
                add: raw.add,
                remove: raw.remove,
                conditions: parse_conditions(&raw.conditions, &place)?,
            };
            check_no_overlap(&conditions, &variant.conditions, &place)?;
            variants.insert(name, variant);
        }

        Ok(Self {
            conditions,
            default,
            variants,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&super::read_file(path)?)
    }

    /// Names of every variant, the default one first.
    pub fn names(&self) -> Vec<&str> {
        std::iter::once(DEFAULT_VARIANT)
            .chain(self.variants.keys().map(String::as_str))
            .collect()
    }

    /// Every inconsistency with the manifest; empty when the variants are sound.
    pub fn validate(&self, manifest: &Manifest) -> Vec<Problem> {
        let known = manifest.keys();
        let mut problems = Vec::new();
        let mut unknown = |variant: &str, keys: &[String]| {
            for key in keys.iter().filter(|k| !known.contains(k.as_str())) {
                problems.push(Problem::UnknownKey {
                    variant: variant.to_owned(),
                    key: key.clone(),
                });
            }
        };
        unknown(DEFAULT_VARIANT, &self.default.remove);
        for (name, variant) in &self.variants {
            unknown(name, &variant.remove);
            unknown(name, &variant.add);
        }
        let default_selection = self.default_selection(manifest);
        for (name, variant) in &self.variants {
            for key in variant
                .add
                .iter()
                .filter(|k| default_selection.contains(k.as_str()))
            {
                problems.push(Problem::AlreadyPresent {
                    variant: name.clone(),
                    key: key.clone(),
                });
            }
        }
        problems
    }

    /// Fails with every inconsistency with the manifest, listed.
    pub fn check(&self, manifest: &Manifest) -> Result<()> {
        let problems = self.validate(manifest);
        if problems.is_empty() {
            return Ok(());
        }
        let list: Vec<String> = problems.iter().map(|p| format!("\n  - {p}")).collect();
        Err(invalid(format!(
            "inconsistent with mods.lock.toml:{}",
            list.concat()
        )))
    }

    /// The mods and conditions of a variant. Fails on an unknown variant, or when
    /// the variants are inconsistent with the manifest.
    pub fn resolve(&self, variant: &str, manifest: &Manifest) -> Result<Resolved> {
        self.check(manifest)?;
        let mut selection = self.default_selection(manifest);
        let own_conditions = if variant == DEFAULT_VARIANT {
            &self.default.conditions
        } else {
            let def = self.variants.get(variant).ok_or_else(|| {
                Error::usage(
                    format!("unknown variant `{variant}`"),
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
            name: variant.to_owned(),
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
struct RawVariants {
    #[serde(default)]
    conditions: toml::Table,
    #[serde(default)]
    default: RawVariant,
    #[serde(default)]
    variants: BTreeMap<String, RawVariant>,
}

/// A variant as written. `extra` catches what serde would otherwise reject
/// blindly, to explain it: a nested variant, a misspelt field.
#[derive(Deserialize, Default)]
struct RawVariant {
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

/// A global condition repeated in a variant would leave two values for one test.
fn check_no_overlap(global: &[Condition], own: &[Condition], place: &str) -> Result<()> {
    match own
        .iter()
        .find(|c| global.iter().any(|g| g.name() == c.name()))
    {
        Some(c) => Err(invalid(format!(
            "condition `{}` is set both at the root, for every variant, and in {place}",
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

    fn variants() -> Variants {
        Variants::load(&fixtures().join(FILE_NAME)).unwrap()
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
        let err = Variants::parse(text).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        err.to_string()
    }

    #[test]
    fn fixture_is_consistent_with_the_manifest() {
        assert_eq!(variants().validate(&manifest()), Vec::new());
        assert_eq!(variants().names(), ["default", "hardcore", "server"]);
    }

    #[test]
    fn default_variant_is_the_manifest_minus_its_removals() {
        let resolved = variants().resolve(DEFAULT_VARIANT, &manifest()).unwrap();
        assert_eq!(resolved.keys, all_but(&["hardcore-tweaks"]));
        assert_eq!(
            resolved.conditions,
            [Condition::DepsComplete, Condition::Side(Side::Client)]
        );
    }

    #[test]
    fn server_variant_removes_from_the_default_selection() {
        let resolved = variants().resolve("server", &manifest()).unwrap();
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
    fn variant_adds_a_mod_removed_from_the_default() {
        let resolved = variants().resolve("hardcore", &manifest()).unwrap();
        assert_eq!(resolved.keys, all_but(&[]));
    }

    #[test]
    fn global_conditions_are_in_every_variant() {
        let variants = variants();
        for name in variants.names() {
            let resolved = variants.resolve(name, &manifest()).unwrap();
            assert_eq!(resolved.conditions.first(), Some(&Condition::DepsComplete));
        }
    }

    #[test]
    fn unknown_variant_is_a_usage_error_listing_the_variants() {
        let err = variants().resolve("lowend", &manifest()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("unknown variant `lowend`"));
        assert!(err.hint().contains("default, hardcore, server"));
    }

    #[test]
    fn unknown_key_is_reported() {
        let variants =
            Variants::parse("[default]\nremove = [\"nope\"]\n[variants.a]\nadd = [\"ghost\"]\n")
                .unwrap();
        assert_eq!(
            variants.validate(&manifest()),
            [
                Problem::UnknownKey {
                    variant: "default".into(),
                    key: "nope".into()
                },
                Problem::UnknownKey {
                    variant: "a".into(),
                    key: "ghost".into()
                },
            ]
        );
        let err = variants.resolve("a", &manifest()).unwrap_err().to_string();
        assert!(err.contains("`ghost`"));
        assert!(err.starts_with("error: variants.toml: inconsistent with mods.lock.toml:"));
        let first_line = err.lines().next().unwrap();
        assert_eq!(first_line.matches("variants.toml").count(), 1, "{err}");
    }

    #[test]
    fn adding_a_key_already_in_the_default_is_reported() {
        let variants = Variants::parse("[variants.a]\nadd = [\"jei\"]\n").unwrap();
        assert_eq!(
            variants.validate(&manifest()),
            [Problem::AlreadyPresent {
                variant: "a".into(),
                key: "jei".into()
            }]
        );
    }

    #[test]
    fn unknown_condition_is_refused() {
        let err = parse_err("[variants.a.conditions]\nfast = true\n");
        assert!(err.contains("unknown condition `fast` in [variants.a]"));
        assert!(parse_err("[conditions]\nfast = true\n").contains("in the root"));
    }

    #[test]
    fn second_level_of_variant_is_refused() {
        let err = parse_err("[variants.server.lowend]\nremove = [\"jei\"]\n");
        assert!(err.contains("[variants.server.lowend] nests a variant in a variant"));
    }

    #[test]
    fn default_variant_cannot_add_nor_be_redefined() {
        assert!(parse_err("[default]\nadd = [\"jei\"]\n").contains("[default] cannot `add`"));
        assert!(parse_err("[default]\nremov = []\n").contains("unknown field `remov`"));
        assert!(parse_err("[variants.default]\n").contains("reserved"));
    }

    #[test]
    fn a_global_condition_cannot_be_repeated_in_a_variant() {
        let err = parse_err(
            "[conditions]\nside = \"client\"\n[variants.a.conditions]\nside = \"server\"\n",
        );
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
        let variants = Variants::parse(
            "[conditions]\nserver-starts = true\nclient-starts = true\ndeps-complete = false\n",
        )
        .unwrap();
        assert_eq!(
            variants.conditions,
            [
                Condition::ClientStarts,
                Condition::ServerStarts {
                    timeout: DEFAULT_SERVER_TIMEOUT
                },
            ]
        );
    }

    #[test]
    fn reference_doc_example_is_valid_and_names_every_condition() {
        let doc = include_str!("../../docs/variants.md");
        let example = doc
            .split("```toml\n")
            .nth(1)
            .and_then(|rest| rest.split("\n```").next())
            .unwrap();
        let variants = Variants::parse(example).unwrap();
        assert_eq!(variants.validate(&manifest()), Vec::new());
        assert_eq!(variants.names(), ["default", "hardcore", "server"]);
        for name in crate::conditions::NAMES {
            assert!(
                doc.contains(&format!("| `{name}` |")),
                "{name} undocumented"
            );
        }
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
    fn empty_file_is_a_default_variant_with_every_mod() {
        let resolved = Variants::parse("")
            .unwrap()
            .resolve(DEFAULT_VARIANT, &manifest())
            .unwrap();
        assert_eq!(resolved.keys, all_but(&[]));
        assert!(resolved.conditions.is_empty());
    }
}
