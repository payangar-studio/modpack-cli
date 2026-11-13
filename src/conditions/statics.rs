//! Conditions read from the manifest alone: `side` and `deps-complete`.

use std::collections::{BTreeMap, BTreeSet};

use super::Status;
use crate::model::manifest::{Manifest, Support};
use crate::model::variants::{Resolved, Side};

/// Fields a platform table names its project by: `project_id` for Modrinth,
/// `projectID` for CurseForge, `id` for the test platform. A table's
/// `dependencies` lists such ids, on the same platform.
const PROJECT_ID_FIELDS: [&str; 3] = ["project_id", "projectID", "id"];

/// Every mod of the variant accepts `side`, after its `env`.
pub fn side(side: Side, manifest: &Manifest, variant: &Resolved) -> Status {
    let (name, refused): (&str, Vec<&str>) = match side {
        Side::Client => (
            "client",
            refusing(manifest, variant, |e| e.env.client == Support::Unsupported),
        ),
        Side::Server => (
            "server",
            refusing(manifest, variant, |e| e.env.server == Support::Unsupported),
        ),
    };
    if refused.is_empty() {
        Status::Passed
    } else {
        Status::Failed(format!(
            "not supported on the {name} side, after their env in mods.lock.toml: {}",
            refused.join(", ")
        ))
    }
}

fn refusing<'m>(
    manifest: &'m Manifest,
    variant: &Resolved,
    unsupported: impl Fn(&crate::model::manifest::Entry) -> bool,
) -> Vec<&'m str> {
    variant
        .keys
        .iter()
        .filter_map(|k| manifest.get(k))
        .filter(|e| unsupported(e))
        .map(|e| e.key.as_str())
        .collect()
}

/// The required dependencies every mod of the variant lists in its platform
/// tables are mods of the variant.
pub fn deps_complete(manifest: &Manifest, variant: &Resolved) -> Status {
    // Platform name, then project id, to manifest key.
    let mut projects: BTreeMap<&str, BTreeMap<String, &str>> = BTreeMap::new();
    for entry in &manifest.mods {
        for (platform, table) in &entry.platforms {
            if let Some(id) = PROJECT_ID_FIELDS
                .iter()
                .find_map(|f| table.get(*f).and_then(id_text))
            {
                projects
                    .entry(platform)
                    .or_default()
                    .insert(id, entry.key.as_str());
            }
        }
    }

    let in_variant: BTreeSet<&str> = variant.keys.iter().map(String::as_str).collect();
    let mut missing = BTreeSet::new();
    for entry in variant.keys.iter().filter_map(|k| manifest.get(k)) {
        for (platform, table) in &entry.platforms {
            let Some(deps) = table.get("dependencies").and_then(toml::Value::as_array) else {
                continue;
            };
            for id in deps.iter().filter_map(id_text) {
                match projects.get(platform.as_str()).and_then(|p| p.get(&id)) {
                    Some(&key) if key == entry.key || in_variant.contains(key) => {}
                    Some(&key) => {
                        missing.insert(format!(
                            "`{}` needs `{key}`, which is not in this variant",
                            entry.key
                        ));
                    }
                    None => {
                        missing.insert(format!(
                            "`{}` needs the {platform} project `{id}`, which no mod of mods.lock.toml is",
                            entry.key
                        ));
                    }
                }
            }
        }
    }
    if missing.is_empty() {
        Status::Passed
    } else {
        Status::Failed(missing.into_iter().collect::<Vec<_>>().join("\n"))
    }
}

/// A project id as text: Modrinth writes strings, CurseForge integers.
fn id_text(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::model::variants::{DEFAULT_VARIANT, Variants};
    use std::path::Path;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
    }

    pub(in crate::conditions) fn manifest() -> Manifest {
        Manifest::load(&fixtures().join(crate::model::manifest::FILE_NAME)).unwrap()
    }

    fn fixture_variants() -> Variants {
        Variants::load(&fixtures().join(crate::model::variants::FILE_NAME)).unwrap()
    }

    fn resolve(variants: &Variants, variant: &str) -> Resolved {
        variants.resolve(variant, &manifest()).unwrap()
    }

    fn failure(status: Status) -> String {
        match status {
            Status::Failed(reason) => reason,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn fixture_variants_pass_their_static_conditions() {
        let variants = fixture_variants();
        let manifest = manifest();
        for name in variants.names() {
            let variant = resolve(&variants, name);
            for condition in &variant.conditions {
                let status = match condition {
                    crate::model::variants::Condition::Side(s) => side(*s, &manifest, &variant),
                    crate::model::variants::Condition::DepsComplete => {
                        deps_complete(&manifest, &variant)
                    }
                    _ => continue,
                };
                assert_eq!(status, Status::Passed, "{name}: {condition:?}");
            }
        }
    }

    #[test]
    fn side_fails_on_the_mods_that_refuse_it() {
        let variant = resolve(&fixture_variants(), DEFAULT_VARIANT);
        let reason = failure(side(Side::Server, &manifest(), &variant));
        assert!(reason.contains("server side"), "{reason}");
        assert!(reason.ends_with(": iris, sodium"), "{reason}");
        assert_eq!(side(Side::Client, &manifest(), &variant), Status::Passed);
    }

    #[test]
    fn global_deps_complete_fails_in_a_variant_missing_a_dependency() {
        // iris requires the Modrinth project AANobbMI, which is sodium.
        let variants = Variants::parse(
            "[conditions]\ndeps-complete = true\n[variants.lowend]\nremove = [\"sodium\"]\n",
        )
        .unwrap();
        let variant = resolve(&variants, "lowend");
        assert_eq!(
            variant.conditions,
            [crate::model::variants::Condition::DepsComplete]
        );
        let reason = failure(deps_complete(&manifest(), &variant));
        assert_eq!(
            reason,
            "`iris` needs `sodium`, which is not in this variant"
        );
        let default = resolve(&variants, DEFAULT_VARIANT);
        assert_eq!(deps_complete(&manifest(), &default), Status::Passed);
    }

    #[test]
    fn dependency_unknown_to_the_manifest_is_named_with_its_platform() {
        let mut manifest = manifest();
        let jei = manifest.get_mut("jei").unwrap();
        jei.platform_mut("curseforge").insert(
            "dependencies".into(),
            toml::Value::Array(vec![306612.into()]),
        );
        let variant = Resolved {
            name: "m".into(),
            keys: vec!["jei".into()],
            conditions: Vec::new(),
        };
        let reason = failure(deps_complete(&manifest, &variant));
        assert_eq!(
            reason,
            "`jei` needs the curseforge project `306612`, which no mod of mods.lock.toml is"
        );
    }

    #[test]
    fn curseforge_integer_ids_match_other_entries() {
        let mut manifest = manifest();
        // jei's CurseForge project id is 238222.
        manifest
            .get_mut("ferritecore")
            .unwrap()
            .platform_mut("curseforge")
            .insert(
                "dependencies".into(),
                toml::Value::Array(vec![238222.into()]),
            );
        let mut variant = Resolved {
            name: "m".into(),
            keys: vec!["ferritecore".into(), "jei".into()],
            conditions: Vec::new(),
        };
        assert_eq!(deps_complete(&manifest, &variant), Status::Passed);
        variant.keys.pop();
        assert_eq!(
            failure(deps_complete(&manifest, &variant)),
            "`ferritecore` needs `jei`, which is not in this variant"
        );
    }
}
