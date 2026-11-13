//! Building manifest entries from the jars of `mods/`, and regenerating the
//! manifest from what `mods/` holds.
//!
//! A jar missing from `mods/` means "not installed on this machine", never
//! "removed from the pack": its entry stays. Only `modpack remove` takes a mod
//! out of the pack.
//!
//! An entry is built from its jar: hashes first, then every platform of the
//! registry is asked to identify the jar, and each platform that knows it
//! completes the entry. A platform that fails (no network, no API key) is
//! skipped with a warning: the entry keeps what the others gave.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Context, Result};
use crate::model::manifest::{self, Entry, Env, Hashes, Manifest, Pack};
use crate::model::socle::SocleConfig;
use crate::platform::{Jar, Registry};
use crate::store::hash::{JarHashes, hash_file};
use crate::store::inventory::inventory;

/// An entry being built, before it gets its key in the manifest.
#[derive(Debug, Clone)]
pub struct Draft {
    pub entry: Entry,
    /// Key to keep: the entry replaces the one with this key.
    pub key: Option<String>,
    /// Project slug given by each platform that knows the jar.
    pub slugs: BTreeMap<String, Option<String>>,
}

impl Draft {
    /// A draft for the jar at `path` (`mods/<name>`), from its hashes.
    pub fn from_hashes(path: &str, hashes: &JarHashes, key: Option<String>) -> Self {
        Self {
            entry: Entry {
                key: key.clone().unwrap_or_else(|| file_name(path).to_owned()),
                path: path.to_owned(),
                file_size: hashes.size,
                downloads: Vec::new(),
                url: None,
                hashes: Hashes {
                    sha1: hashes.sha1.clone(),
                    sha512: hashes.sha512.clone(),
                },
                env: Env::default(),
                platforms: BTreeMap::new(),
            },
            key,
            slugs: BTreeMap::new(),
        }
    }

    /// A draft for the jar at `path` under `root`, hashing it.
    pub fn from_jar(root: &Path, path: &str, key: Option<String>) -> Result<Self> {
        let file = root.join(path);
        let hashes = hash_file(&file).context(format!("reading {}", file.display()))?;
        Ok(Self::from_hashes(path, &hashes, key))
    }

    /// Whether some platform knows the mod.
    pub fn has_source(&self) -> bool {
        !self.entry.platforms.is_empty()
    }

    /// Puts the entry in the manifest under its kept key, or a new key built
    /// from the platforms' slugs (in registry order) or the jar name. Returns
    /// the key.
    pub fn commit(mut self, manifest: &mut Manifest, registry: &Registry) -> String {
        let key = self.key.take().unwrap_or_else(|| {
            let slugs: Vec<Option<&str>> = registry
                .names()
                .into_iter()
                .map(|name| self.slugs.get(name).and_then(|s| s.as_deref()))
                .collect();
            manifest.new_key(&slugs, file_name(&self.entry.path))
        });
        self.entry.key = key.clone();
        manifest.upsert(self.entry);
        key
    }
}

/// Asks every platform without a table in a draft to identify its jar, then
/// lets each platform that recognized one complete the entry. Returns the
/// warnings to show.
pub fn complete(root: &Path, drafts: &mut [Draft], registry: &Registry) -> Vec<String> {
    let mut warnings = Vec::new();
    for platform in registry.all() {
        let name = platform.name();
        let pending: Vec<usize> = (0..drafts.len())
            .filter(|&i| drafts[i].entry.platform(name).is_none())
            .collect();
        if pending.is_empty() {
            continue;
        }
        let jars: Vec<Jar> = pending
            .iter()
            .map(|&i| {
                let e = &drafts[i].entry;
                Jar {
                    path: root.join(&e.path),
                    sha1: e.hashes.sha1.clone(),
                    sha512: e.hashes.sha512.clone(),
                    size: e.file_size,
                }
            })
            .collect();
        let found = match platform.identify(&jars) {
            Ok(found) => found,
            Err(err) => {
                warnings.push(format!("{name} skipped: {}", first_line(&err)));
                continue;
            }
        };
        for (&i, found) in pending.iter().zip(found) {
            let Some(found) = found else { continue };
            let draft = &mut drafts[i];
            if !found.exact {
                warnings.push(format!(
                    "{} matches a {name} file only partly; not linked to {name}",
                    draft.entry.path
                ));
                continue;
            }
            draft.entry.platforms.insert(name.to_owned(), found.section);
            draft.slugs.insert(name.to_owned(), found.slug);
            if let Err(err) = platform.fill_entry(&mut draft.entry) {
                warnings.push(format!(
                    "{name} could not complete {}: {}",
                    draft.entry.path,
                    first_line(&err)
                ));
            }
        }
    }
    warnings
}

/// The manifest of the repository, or an empty one for the pack described in
/// `.modpack/socle.toml` when there is none yet.
pub fn load_or_new(root: &Path) -> Result<Manifest> {
    let path = root.join(manifest::FILE_NAME);
    if path.is_file() {
        return Manifest::load(&path);
    }
    let init = SocleConfig::load(root)?.init;
    Ok(Manifest::new(Pack {
        minecraft: init.minecraft,
        loader: init.loader,
        loader_version: init.loader_version,
    }))
}

/// What a regeneration changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    /// Keys of the entries built for new jars.
    pub added: Vec<String>,
    /// Keys of the entries whose jar is missing from `mods/`, kept as they
    /// are: not installed here does not mean removed from the pack.
    pub missing: Vec<String>,
    /// Keys of the entries whose jar changed, rebuilt under the same key.
    pub rebuilt: Vec<String>,
    /// Keys of the new or rebuilt entries no platform knows.
    pub unidentified: Vec<String>,
    pub warnings: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.rebuilt.is_empty()
    }
}

/// Rewrites the manifest from `mods/`: new jars are identified and added,
/// entries whose jar changed are rebuilt under the same key, entries whose jar
/// is missing are kept and reported. Writes the file only when something
/// changed, or when it did not exist.
pub fn regenerate(root: &Path, registry: &Registry) -> Result<Changes> {
    let path = root.join(manifest::FILE_NAME);
    let existed = path.is_file();
    let mut manifest = load_or_new(root)?;
    let inv = inventory(root, &manifest)?;
    let mut changes = Changes {
        missing: inv.missing.clone(),
        ..Changes::default()
    };

    let mut drafts = Vec::new();
    for key in &inv.mismatched {
        let jar = manifest
            .get(key)
            .expect("inventory keys are manifest keys")
            .path
            .clone();
        drafts.push(Draft::from_hashes(&jar, &inv.jars[&jar], Some(key.clone())));
    }
    for jar in &inv.unknown {
        drafts.push(Draft::from_hashes(jar, &inv.jars[jar], None));
    }
    changes.warnings = complete(root, &mut drafts, registry);
    for draft in drafts {
        let rebuilt = draft.key.is_some();
        let source = draft.has_source();
        let key = draft.commit(&mut manifest, registry);
        if !source {
            changes.unidentified.push(key.clone());
        }
        if rebuilt {
            changes.rebuilt.push(key);
        } else {
            changes.added.push(key);
        }
    }
    if !existed || !changes.is_empty() {
        manifest.save(&path)?;
    }
    Ok(changes)
}

/// Differences between the manifest and `mods/` that `regenerate` would
/// write, one line each; empty when the manifest is up to date. A missing jar
/// is not one: the entry stays. Reads the disk only, never the network.
pub fn check(root: &Path) -> Result<Vec<String>> {
    let mut gaps = Vec::new();
    if !root.join(manifest::FILE_NAME).is_file() {
        gaps.push(format!("{} does not exist", manifest::FILE_NAME));
    }
    let manifest = load_or_new(root)?;
    let inv = inventory(root, &manifest)?;
    gaps.extend(inv.unknown.iter().map(|p| format!("new jar: {p}")));
    gaps.extend(inv.mismatched.iter().map(|k| {
        let path = &manifest
            .get(k)
            .expect("inventory keys are manifest keys")
            .path;
        format!("jar changed: {k} ({path})")
    }));
    Ok(gaps)
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The first line of an error, without its `error: ` prefix.
pub fn first_line(err: &crate::error::Error) -> String {
    let text = err.to_string();
    let line = text.lines().next().unwrap_or_default();
    line.strip_prefix("error: ").unwrap_or(line).to_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::socle::InitValues;
    use crate::platform::fake::{FakeMod, FakePlatform};
    use std::fs;

    /// An empty pack repository: socle settings and `mods/`, no manifest.
    pub(crate) fn repo() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        SocleConfig {
            version: "0.1.0".into(),
            init: InitValues {
                name: "Test Pack".into(),
                minecraft: "1.21.1".into(),
                loader: "neoforge".into(),
                loader_version: "21.1.77".into(),
                author: "a".into(),
            },
        }
        .save(root.path())
        .unwrap();
        fs::create_dir_all(root.path().join("mods")).unwrap();
        root
    }

    /// Two platforms: `fake` hosts `sodium` and `jei`, `other` hosts `jei`
    /// under another slug.
    pub(crate) fn registry() -> Registry {
        let mut other_jei = FakeMod::new("jei");
        other_jei.slug = "just-enough-items".into();
        Registry::new(vec![
            Box::new(
                FakePlatform::new("fake")
                    .with(FakeMod::new("sodium"))
                    .with(FakeMod::new("jei")),
            ),
            Box::new(FakePlatform::new("other").with(other_jei)),
        ])
    }

    fn put(root: &Path, name: &str, bytes: &[u8]) {
        fs::write(root.join("mods").join(name), bytes).unwrap();
    }

    fn load(root: &Path) -> Manifest {
        Manifest::load(&root.join(manifest::FILE_NAME)).unwrap()
    }

    #[test]
    fn check_fails_on_a_jar_added_by_hand_then_passes_after_regenerating() {
        let root = repo();
        let registry = registry();
        regenerate(root.path(), &registry).unwrap();
        assert_eq!(check(root.path()).unwrap(), Vec::<String>::new());

        put(root.path(), "sodium-1.0.0.jar", b"sodium");
        assert_eq!(
            check(root.path()).unwrap(),
            ["new jar: mods/sodium-1.0.0.jar"]
        );
        let changes = regenerate(root.path(), &registry).unwrap();
        assert_eq!(changes.added, ["sodium"]);
        assert!(check(root.path()).unwrap().is_empty());
    }

    #[test]
    fn new_jar_gets_both_sets_of_ids() {
        let root = repo();
        put(root.path(), "jei-1.0.0.jar", b"jei");
        let changes = regenerate(root.path(), &registry()).unwrap();
        assert_eq!(changes.added, ["jei"]);
        let manifest = load(root.path());
        let jei = manifest.get("jei").unwrap();
        assert_eq!(jei.platform("fake").unwrap()["id"].as_str(), Some("jei"));
        assert_eq!(
            jei.platform("other").unwrap()["id"].as_str(),
            Some("just-enough-items")
        );
        assert!(jei.platform("fake").unwrap().contains_key("dependencies"));
        assert_eq!(jei.file_size, 3);
    }

    #[test]
    fn unknown_jar_gets_an_entry_without_source() {
        let root = repo();
        put(root.path(), "Mystery_Mod-2.1.jar", b"???");
        let changes = regenerate(root.path(), &registry()).unwrap();
        assert_eq!(changes.added, ["mystery-mod"]);
        assert_eq!(changes.unidentified, ["mystery-mod"]);
        let entry = load(root.path()).get("mystery-mod").unwrap().clone();
        assert!(entry.platforms.is_empty() && entry.downloads.is_empty());
        assert_eq!(entry.path, "mods/Mystery_Mod-2.1.jar");
    }

    #[test]
    fn missing_jar_keeps_its_entry_and_changed_jar_is_rebuilt_under_its_key() {
        let root = repo();
        put(root.path(), "sodium-1.0.0.jar", b"sodium");
        put(root.path(), "custom.jar", b"v1");
        let registry = registry();
        regenerate(root.path(), &registry).unwrap();
        let path = root.path().join(manifest::FILE_NAME);

        fs::remove_file(root.path().join("mods/sodium-1.0.0.jar")).unwrap();
        assert!(check(root.path()).unwrap().is_empty());
        let before = fs::read_to_string(&path).unwrap();
        let changes = regenerate(root.path(), &registry).unwrap();
        assert!(changes.is_empty());
        assert_eq!(changes.missing, ["sodium"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), before);

        put(root.path(), "custom.jar", b"jei");
        assert_eq!(
            check(root.path()).unwrap(),
            ["jar changed: custom (mods/custom.jar)"]
        );
        let changes = regenerate(root.path(), &registry).unwrap();
        assert_eq!(changes.missing, ["sodium"]);
        assert_eq!(changes.rebuilt, ["custom"]);
        let manifest = load(root.path());
        assert_eq!(
            manifest.keys().into_iter().collect::<Vec<_>>(),
            ["custom", "sodium"]
        );
        let custom = manifest.get("custom").unwrap();
        assert_eq!(custom.platform("fake").unwrap()["id"].as_str(), Some("jei"));
        assert!(check(root.path()).unwrap().is_empty());
    }

    #[test]
    fn up_to_date_manifest_is_not_rewritten() {
        let root = repo();
        let registry = registry();
        regenerate(root.path(), &registry).unwrap();
        let path = root.path().join(manifest::FILE_NAME);
        fs::write(&path, fs::read_to_string(&path).unwrap() + "\n# kept\n").unwrap();
        assert!(regenerate(root.path(), &registry).unwrap().is_empty());
        assert!(fs::read_to_string(&path).unwrap().ends_with("# kept\n"));
    }

    #[test]
    fn missing_manifest_is_a_gap() {
        let root = repo();
        assert_eq!(
            check(root.path()).unwrap(),
            ["mods.lock.toml does not exist"]
        );
    }

    /// Fails every identification, as CurseForge without an API key does.
    struct Offline;

    impl crate::platform::Platform for Offline {
        fn name(&self) -> &str {
            "offline"
        }
        fn capability(&self, _: crate::platform::Operation) -> crate::platform::Capability {
            crate::platform::Capability::Api
        }
    }

    #[test]
    fn failing_platform_is_skipped_with_a_warning() {
        let root = repo();
        put(root.path(), "sodium-1.0.0.jar", b"sodium");
        let registry = Registry::new(vec![
            Box::new(Offline),
            Box::new(FakePlatform::new("fake").with(FakeMod::new("sodium"))),
        ]);
        let changes = regenerate(root.path(), &registry).unwrap();
        assert_eq!(changes.added, ["sodium"]);
        assert_eq!(changes.warnings.len(), 1);
        assert!(changes.warnings[0].starts_with("offline skipped:"));
    }
}
