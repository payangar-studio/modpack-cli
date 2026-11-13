//! Conditions read from the manifest alone: `side` and `deps-complete`.

use std::collections::{BTreeMap, BTreeSet};

use super::Status;
use crate::model::manifest::{Manifest, Support};
use crate::model::modes::{Resolved, Side};

/// Fields a platform table names its project by: `project_id` for Modrinth,
/// `projectID` for CurseForge, `id` for the test platform. A table's
/// `dependencies` lists such ids, on the same platform.
const PROJECT_ID_FIELDS: [&str; 3] = ["project_id", "projectID", "id"];

/// Every mod of the mode accepts `side`, after its `env`.
pub fn side(side: Side, manifest: &Manifest, mode: &Resolved) -> Status {
    let (name, refused): (&str, Vec<&str>) = match side {
        Side::Client => (
            "client",
            refusing(manifest, mode, |e| e.env.client == Support::Unsupported),
        ),
        Side::Server => (
            "server",
            refusing(manifest, mode, |e| e.env.server == Support::Unsupported),
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
    mode: &Resolved,
    unsupported: impl Fn(&crate::model::manifest::Entry) -> bool,
) -> Vec<&'m str> {
    mode.keys
        .iter()
        .filter_map(|k| manifest.get(k))
        .filter(|e| unsupported(e))
        .map(|e| e.key.as_str())
        .collect()
}

/// The required dependencies every mod of the mode lists in its platform
/// tables are mods of the mode.
pub fn deps_complete(manifest: &Manifest, mode: &Resolved) -> Status {
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

    let in_mode: BTreeSet<&str> = mode.keys.iter().map(String::as_str).collect();
    let mut missing = BTreeSet::new();
    for entry in mode.keys.iter().filter_map(|k| manifest.get(k)) {
        for (platform, table) in &entry.platforms {
            let Some(deps) = table.get("dependencies").and_then(toml::Value::as_array) else {
                continue;
            };
            for id in deps.iter().filter_map(id_text) {
                match projects.get(platform.as_str()).and_then(|p| p.get(&id)) {
                    Some(&key) if key == entry.key || in_mode.contains(key) => {}
                    Some(&key) => {
                        missing.insert(format!(
                            "`{}` needs `{key}`, which is not in this mode",
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
    use crate::model::modes::{DEFAULT_MODE, Modes};
    use std::path::Path;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
    }

    pub(in crate::conditions) fn manifest() -> Manifest {
        Manifest::load(&fixtures().join(crate::model::manifest::FILE_NAME)).unwrap()
    }

    fn fixture_modes() -> Modes {
        Modes::load(&fixtures().join(crate::model::modes::FILE_NAME)).unwrap()
    }

    fn resolve(modes: &Modes, mode: &str) -> Resolved {
        modes.resolve(mode, &manifest()).unwrap()
    }

    fn failure(status: Status) -> String {
        match status {
            Status::Failed(reason) => reason,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn fixture_modes_pass_their_static_conditions() {
        let modes = fixture_modes();
        let manifest = manifest();
        for name in modes.names() {
            let mode = resolve(&modes, name);
            for condition in &mode.conditions {
                let status = match condition {
                    crate::model::modes::Condition::Side(s) => side(*s, &manifest, &mode),
                    crate::model::modes::Condition::DepsComplete => deps_complete(&manifest, &mode),
                    _ => continue,
                };
                assert_eq!(status, Status::Passed, "{name}: {condition:?}");
            }
        }
    }

    #[test]
    fn side_fails_on_the_mods_that_refuse_it() {
        let mode = resolve(&fixture_modes(), DEFAULT_MODE);
        let reason = failure(side(Side::Server, &manifest(), &mode));
        assert!(reason.contains("server side"), "{reason}");
        assert!(reason.ends_with(": iris, sodium"), "{reason}");
        assert_eq!(side(Side::Client, &manifest(), &mode), Status::Passed);
    }

    #[test]
    fn global_deps_complete_fails_in_a_mode_missing_a_dependency() {
        // iris requires the Modrinth project AANobbMI, which is sodium.
        let modes = Modes::parse(
            "[conditions]\ndeps-complete = true\n[modes.lowend]\nremove = [\"sodium\"]\n",
        )
        .unwrap();
        let mode = resolve(&modes, "lowend");
        assert_eq!(
            mode.conditions,
            [crate::model::modes::Condition::DepsComplete]
        );
        let reason = failure(deps_complete(&manifest(), &mode));
        assert_eq!(reason, "`iris` needs `sodium`, which is not in this mode");
        let default = resolve(&modes, DEFAULT_MODE);
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
        let mode = Resolved {
            name: "m".into(),
            keys: vec!["jei".into()],
            conditions: Vec::new(),
        };
        let reason = failure(deps_complete(&manifest, &mode));
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
        let mut mode = Resolved {
            name: "m".into(),
            keys: vec!["ferritecore".into(), "jei".into()],
            conditions: Vec::new(),
        };
        assert_eq!(deps_complete(&manifest, &mode), Status::Passed);
        mode.keys.pop();
        assert_eq!(
            failure(deps_complete(&manifest, &mode)),
            "`ferritecore` needs `jei`, which is not in this mode"
        );
    }
}
