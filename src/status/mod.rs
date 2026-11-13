//! The state of a pack on this machine, as `modpack status` reports it: socle
//! version, the pack's instance, the mode loaded there, drift, jars unknown to
//! the manifest, and how `mods/` of the repository compares to the manifest.
//!
//! Computing it reads files only: nothing is written, nothing goes to the
//! network. The JSON form is a contract, documented in `docs/status.md`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Context, Result};
use crate::model::instance::{InstanceState, Instances, drift};
use crate::model::manifest::{self, Manifest};
use crate::model::socle::{SocleConfig, VersionCheck, cli_version, compare_versions};
use crate::store::MODS_DIR;
use crate::store::hash::hash_file;
use crate::store::inventory::inventory;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub socle: Socle,
    /// The pack's instance on this machine; `None` when there is none.
    pub instance: Option<Instance>,
    /// Manifest entries whose jar is absent from `mods/` of the repository.
    pub missing_jars: Vec<Jar>,
    pub manifest: ManifestCheck,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Socle {
    /// Socle version of the repository.
    pub repository: String,
    /// Version of this CLI.
    pub cli: String,
    pub gap: Gap,
}

/// The repository's socle version compared to the CLI's. A newer repository
/// is refused before any report, as by every command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gap {
    Equal,
    /// `modpack update` brings the repository to the CLI's version.
    Older,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Instance {
    pub path: String,
    /// Mode loaded last; `None` before the first load.
    pub mode: Option<String>,
    /// Files the CLI wrote, changed or gone since, relative to the instance.
    pub drift: Vec<String>,
    /// Jars of the instance's `mods/` whose hashes no manifest entry has.
    pub unknown_jars: Vec<UnknownJar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownJar {
    /// Relative to the instance, such as `mods/foo.jar`.
    pub path: String,
    pub sha1: String,
    pub sha512: String,
}

/// A manifest entry and the path of its jar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Jar {
    pub key: String,
    pub path: String,
}

/// What `modpack manifest --check` would find, by hashes only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestCheck {
    /// Whether `new_jars` or `changed` is not empty.
    pub stale: bool,
    /// Jars of `mods/` that no entry lists, as `mods/<name>`.
    pub new_jars: Vec<String>,
    /// Entries whose jar is in `mods/` with other hashes.
    pub changed: Vec<Jar>,
}

/// The status of the repository at `root`, with the machine's instances file.
pub fn status(root: &Path, instances_file: &Path) -> Result<Status> {
    let config = SocleConfig::load(root)?;
    let cli = cli_version();
    let check = compare_versions(&config.version, &cli)?;
    check.enforce()?;
    let gap = match check {
        VersionCheck::Equal => Gap::Equal,
        _ => Gap::Older,
    };

    let manifest = Manifest::load(&root.join(manifest::FILE_NAME))?;
    let inv = inventory(root, &manifest)?;
    let jars = |keys: &[String]| -> Vec<Jar> {
        keys.iter()
            .filter_map(|k| manifest.get(k))
            .map(|e| Jar {
                key: e.key.clone(),
                path: e.path.clone(),
            })
            .collect()
    };
    let changed = jars(&inv.mismatched);

    let instance = match Instances::load(instances_file)?
        .get(&config.init.name)
        .filter(|dir| dir.is_dir())
    {
        Some(dir) => Some(instance(dir, &manifest)?),
        None => None,
    };

    Ok(Status {
        socle: Socle {
            repository: config.version,
            cli: cli.to_string(),
            gap,
        },
        instance,
        missing_jars: jars(&inv.missing),
        manifest: ManifestCheck {
            stale: !inv.unknown.is_empty() || !changed.is_empty(),
            new_jars: inv.unknown,
            changed,
        },
    })
}

fn instance(dir: &Path, manifest: &Manifest) -> Result<Instance> {
    let state = InstanceState::load(dir)?;
    let drift = drift(dir, &state)?
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut unknown_jars = Vec::new();
    for (path, relative) in jars_of(&dir.join(MODS_DIR))? {
        let hashes = hash_file(&path).context(format!("reading {}", path.display()))?;
        if !manifest.mods.iter().any(|e| hashes.matches(&e.hashes)) {
            unknown_jars.push(UnknownJar {
                path: relative,
                sha1: hashes.sha1,
                sha512: hashes.sha512,
            });
        }
    }
    Ok(Instance {
        path: dir.to_string_lossy().into_owned(),
        mode: state.mode,
        drift,
        unknown_jars,
    })
}

/// The `.jar` files of a directory, with their path as `mods/<name>`, sorted.
/// A missing directory holds none.
fn jars_of(dir: &Path) -> Result<Vec<(PathBuf, String)>> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(format!("reading {}", dir.display())),
    };
    let mut jars = Vec::new();
    for item in listing {
        let item = item.context(format!("reading {}", dir.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.to_ascii_lowercase().ends_with(".jar") && item.path().is_file() {
            jars.push((item.path(), format!("{MODS_DIR}/{name}")));
        }
    }
    jars.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(jars)
}

impl Status {
    /// The report for a human, with what to do about each finding.
    pub fn write_text(&self, out: &mut dyn Write) -> io::Result<()> {
        let s = &self.socle;
        match s.gap {
            Gap::Equal => writeln!(out, "socle: {} (same as the CLI)", s.repository)?,
            Gap::Older => writeln!(
                out,
                "socle: {}, older than the CLI ({}); run `modpack update`",
                s.repository, s.cli
            )?,
        }

        match &self.instance {
            None => writeln!(
                out,
                "instance: none on this machine; run `modpack setup` to create it"
            )?,
            Some(i) => {
                writeln!(out, "instance: {}", i.path)?;
                match &i.mode {
                    Some(mode) => writeln!(out, "mode: {mode}")?,
                    None => writeln!(out, "mode: none loaded; run `modpack mode load <mode>`")?,
                }
                list(
                    out,
                    "drift, files changed in the instance since the CLI wrote them",
                    &i.drift,
                )?;
                if !i.unknown_jars.is_empty() {
                    writeln!(
                        out,
                        "jars in the instance unknown to the manifest ({}):",
                        i.unknown_jars.len()
                    )?;
                    for jar in &i.unknown_jars {
                        writeln!(out, "  - {} (sha1 {})", jar.path, jar.sha1)?;
                    }
                }
            }
        }

        let missing: Vec<String> = self.missing_jars.iter().map(Jar::describe).collect();
        list(
            out,
            "jars missing from mods/, run `modpack install`",
            &missing,
        )?;

        let m = &self.manifest;
        if m.stale {
            writeln!(out, "manifest: stale; run `modpack manifest`")?;
            list(out, "jars of mods/ not in the manifest", &m.new_jars)?;
            let changed: Vec<String> = m.changed.iter().map(Jar::describe).collect();
            list(out, "jars of mods/ changed since the manifest", &changed)?;
        } else {
            writeln!(out, "manifest: up to date with mods/")?;
        }
        Ok(())
    }
}

impl Jar {
    fn describe(&self) -> String {
        format!("{} ({})", self.key, self.path)
    }
}

/// A titled list, written only when it is not empty.
fn list(out: &mut dyn Write, title: &str, items: &[String]) -> io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(out, "{title} ({}):", items.len())?;
    for item in items {
        writeln!(out, "  - {item}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::model::socle::InitValues;
    use crate::store::download::tests::{entry, manifest};
    use std::collections::BTreeMap;

    const PACK: &str = "Test Pack";

    /// A repository whose manifest lists `jei` and `iris`, both in `mods/`;
    /// an instance where the CLI loaded the `default` mode with both jars and
    /// one config file; and the instances file that links them.
    struct Setup {
        repo: tempfile::TempDir,
        machine: tempfile::TempDir,
    }

    impl Setup {
        fn new() -> Self {
            let repo = tempfile::tempdir().unwrap();
            SocleConfig {
                version: cli_version().to_string(),
                init: InitValues {
                    name: PACK.into(),
                    minecraft: "1.21.1".into(),
                    loader: "neoforge".into(),
                    loader_version: "21.1.77".into(),
                    author: "a".into(),
                },
            }
            .save(repo.path())
            .unwrap();
            manifest(vec![entry("jei", b"jei"), entry("iris", b"iris")])
                .save(&repo.path().join(manifest::FILE_NAME))
                .unwrap();
            let setup = Self {
                repo,
                machine: tempfile::tempdir().unwrap(),
            };
            for key in ["jei", "iris"] {
                setup.write_repo(&format!("mods/{key}-1.0.0.jar"), key.as_bytes());
                setup.write_instance(&format!("mods/{key}-1.0.0.jar"), key.as_bytes());
            }
            setup.write_instance("config/jei.toml", b"a = 1");
            let mut state = InstanceState {
                mode: Some("default".into()),
                ..Default::default()
            };
            for file in [
                "mods/jei-1.0.0.jar",
                "mods/iris-1.0.0.jar",
                "config/jei.toml",
            ] {
                state.record(&setup.instance(), file).unwrap();
            }
            state.save(&setup.instance()).unwrap();
            let mut instances = Instances::default();
            instances.set(PACK, setup.instance());
            instances.save(&setup.instances_file()).unwrap();
            setup
        }

        fn instance(&self) -> PathBuf {
            self.machine.path().join("profiles").join(PACK)
        }

        fn instances_file(&self) -> PathBuf {
            self.machine.path().join("modpack/instances.toml")
        }

        fn write_repo(&self, relative: &str, bytes: &[u8]) {
            write(&self.repo.path().join(relative), bytes);
        }

        fn write_instance(&self, relative: &str, bytes: &[u8]) {
            write(&self.instance().join(relative), bytes);
        }

        fn status(&self) -> Status {
            status(self.repo.path(), &self.instances_file()).unwrap()
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn text(status: &Status) -> String {
        let mut out = Vec::new();
        status.write_text(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Every file under `dir` with its content.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for item in fs::read_dir(&dir).unwrap() {
                let path = item.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        files
    }

    #[test]
    fn clean_state_reports_nothing_to_do() {
        let setup = Setup::new();
        let status = setup.status();
        assert_eq!(status.socle.gap, Gap::Equal);
        let instance = status.instance.as_ref().unwrap();
        assert_eq!(instance.path, setup.instance().to_string_lossy());
        assert_eq!(instance.mode.as_deref(), Some("default"));
        assert!(instance.drift.is_empty());
        assert!(instance.unknown_jars.is_empty());
        assert!(status.missing_jars.is_empty());
        assert!(!status.manifest.stale);

        let out = text(&status);
        assert!(out.contains("mode: default"), "{out}");
        assert!(out.contains("manifest: up to date"), "{out}");
        assert!(!out.contains("drift"), "{out}");
    }

    #[test]
    fn drift_lists_changed_and_deleted_files() {
        let setup = Setup::new();
        setup.write_instance("config/jei.toml", b"a = 2");
        fs::remove_file(setup.instance().join("mods/iris-1.0.0.jar")).unwrap();
        let status = setup.status();
        assert_eq!(
            status.instance.as_ref().unwrap().drift,
            ["config/jei.toml", "mods/iris-1.0.0.jar"]
        );
        assert!(text(&status).contains("drift, files changed in the instance"));
    }

    #[test]
    fn unknown_jar_of_the_instance_comes_with_its_hashes() {
        let setup = Setup::new();
        setup.write_instance("mods/tried-in-the-app.jar", b"abc");
        // A manifest jar under another name is known by its hashes.
        setup.write_instance("mods/jei-renamed.jar", b"jei");
        let status = setup.status();
        let instance = status.instance.as_ref().unwrap();
        assert_eq!(
            instance.unknown_jars,
            [UnknownJar {
                path: "mods/tried-in-the-app.jar".into(),
                sha1: "a9993e364706816aba3e25717850c26c9cd0d89d".into(),
                sha512: "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
                    .into(),
            }]
        );
        assert!(instance.drift.is_empty());
        assert!(
            !status.manifest.stale,
            "the instance does not feed the manifest"
        );
    }

    #[test]
    fn missing_jar_is_reported_without_making_the_manifest_stale() {
        let setup = Setup::new();
        fs::remove_file(setup.repo.path().join("mods/iris-1.0.0.jar")).unwrap();
        let status = setup.status();
        assert_eq!(
            status.missing_jars,
            [Jar {
                key: "iris".into(),
                path: "mods/iris-1.0.0.jar".into()
            }]
        );
        assert!(!status.manifest.stale);
        assert!(text(&status).contains("missing from mods/, run `modpack install`"));
    }

    #[test]
    fn manifest_is_stale_on_a_new_or_changed_jar_of_mods() {
        let setup = Setup::new();
        setup.write_repo("mods/added-by-hand.jar", b"x");
        setup.write_repo("mods/jei-1.0.0.jar", b"edited");
        let status = setup.status();
        assert!(status.manifest.stale);
        assert_eq!(status.manifest.new_jars, ["mods/added-by-hand.jar"]);
        assert_eq!(
            status.manifest.changed,
            [Jar {
                key: "jei".into(),
                path: "mods/jei-1.0.0.jar".into()
            }]
        );
        assert!(text(&status).contains("manifest: stale; run `modpack manifest`"));
    }

    #[test]
    fn no_instance_on_this_machine() {
        let setup = Setup::new();
        fs::remove_file(setup.instances_file()).unwrap();
        let status = setup.status();
        assert_eq!(status.instance, None);
        assert!(text(&status).contains("instance: none on this machine"));
    }

    #[test]
    fn registered_instance_whose_directory_is_gone_counts_as_none() {
        let setup = Setup::new();
        fs::remove_dir_all(setup.instance()).unwrap();
        assert_eq!(setup.status().instance, None);
    }

    #[test]
    fn instance_without_a_loaded_mode() {
        let setup = Setup::new();
        fs::remove_file(setup.instance().join(crate::model::instance::STATE_FILE)).unwrap();
        let status = setup.status();
        let instance = status.instance.as_ref().unwrap();
        assert_eq!(instance.mode, None);
        assert!(instance.drift.is_empty());
        assert!(text(&status).contains("mode: none loaded"));
    }

    #[test]
    fn older_socle_is_reported_and_newer_refused() {
        let setup = Setup::new();
        let mut config = SocleConfig::load(setup.repo.path()).unwrap();
        config.version = "0.0.1".into();
        config.save(setup.repo.path()).unwrap();
        let status = setup.status();
        assert_eq!(status.socle.gap, Gap::Older);
        assert!(text(&status).contains("run `modpack update`"));

        config.version = "999.0.0".into();
        config.save(setup.repo.path()).unwrap();
        let err = status_err(&setup);
        assert_eq!(err.kind(), ErrorKind::Usage);
    }

    fn status_err(setup: &Setup) -> crate::error::Error {
        status(setup.repo.path(), &setup.instances_file()).unwrap_err()
    }

    #[test]
    fn writes_nothing() {
        let setup = Setup::new();
        setup.write_instance("config/jei.toml", b"a = 2");
        setup.write_instance("mods/tried.jar", b"x");
        setup.write_repo("mods/added.jar", b"y");
        let before = snapshot(setup.repo.path());
        let machine_before = snapshot(setup.machine.path());
        let status = setup.status();
        text(&status);
        assert_eq!(snapshot(setup.repo.path()), before);
        assert_eq!(snapshot(setup.machine.path()), machine_before);
    }

    /// The JSON Schema documented in `docs/status.md`, and its example.
    fn documented() -> (serde_json::Value, serde_json::Value) {
        let doc = include_str!("../../docs/status.md");
        let mut blocks = doc.split("```json\n").skip(1).map(|rest| {
            let block = rest.split("\n```").next().unwrap();
            serde_json::from_str::<serde_json::Value>(block).unwrap()
        });
        (blocks.next().unwrap(), blocks.next().unwrap())
    }

    fn assert_valid(schema: &serde_json::Value, value: &serde_json::Value) {
        let validator = jsonschema::validator_for(schema).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(value)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{errors:#?}\n{value:#}");
    }

    #[test]
    fn json_output_follows_the_documented_schema() {
        let (schema, example) = documented();
        assert_valid(&schema, &example);

        let setup = Setup::new();
        assert_valid(&schema, &serde_json::to_value(setup.status()).unwrap());

        setup.write_instance("config/jei.toml", b"a = 2");
        setup.write_instance("mods/tried.jar", b"x");
        setup.write_repo("mods/added.jar", b"y");
        setup.write_repo("mods/jei-1.0.0.jar", b"edited");
        fs::remove_file(setup.repo.path().join("mods/iris-1.0.0.jar")).unwrap();
        let busy = serde_json::to_value(setup.status()).unwrap();
        assert_valid(&schema, &busy);
        assert_eq!(
            busy["instance"]["unknown_jars"][0]["path"],
            "mods/tried.jar"
        );

        fs::remove_file(setup.instances_file()).unwrap();
        let none = serde_json::to_value(setup.status()).unwrap();
        assert_valid(&schema, &none);
        assert!(none["instance"].is_null());
    }

    #[test]
    fn schema_rejects_what_breaks_the_contract() {
        let (schema, example) = documented();
        let mut extra = example.clone();
        extra["surprise"] = true.into();
        let mut renamed = example.clone();
        renamed["manifest"]["stale"] = "no".into();
        for bad in [extra, renamed] {
            let validator = jsonschema::validator_for(&schema).unwrap();
            assert!(!validator.is_valid(&bad), "{bad:#}");
        }
    }
}
