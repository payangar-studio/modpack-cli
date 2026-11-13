//! Fetching the jar of a manifest entry into `mods/`.
//!
//! Sources are tried in order: the entry's `downloads`, then every platform of
//! the registry that has a table in the entry, then the direct `url`. What a
//! source sends is written to a temporary file next to the jar, checked
//! against the manifest's sha1 and sha512, and only then renamed to the jar's
//! name: a partial or wrong download never sits under the final name.

use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use super::hash::copy_hashing;
use super::{jar_path, write_atomic};
use crate::error::{Context, Error, Result};
use crate::model::installed::Installed;
use crate::model::manifest::Entry;
use crate::platform::{Download, Registry};

/// How many jars are downloaded at the same time.
pub const PARALLEL: usize = 8;

/// Opens a URL for reading.
pub trait Fetch: Sync {
    fn get(&self, url: &str) -> Result<Box<dyn Read + Send>>;
}

/// [`Fetch`] over HTTP.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(concat!(
                "payangar-studio/modpack-cli/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .new_agent();
        Self { agent }
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for Http {
    fn get(&self, url: &str) -> Result<Box<dyn Read + Send>> {
        let response = self.agent.get(url).call().map_err(|e| {
            Error::internal(
                format!("cannot download {url}"),
                "check the network connection; if the URL is gone, regenerate the manifest with `modpack manifest`",
            )
            .with_source(e)
        })?;
        Ok(Box::new(response.into_body().into_reader()))
    }
}

/// Where jars come from.
#[derive(Clone, Copy)]
pub struct Sources<'a> {
    pub registry: &'a Registry,
    pub fetch: &'a dyn Fetch,
}

/// A jar no source could provide.
#[derive(Debug)]
pub struct Failure {
    pub key: String,
    /// Every source tried, in order, with why it failed.
    pub attempts: Vec<Attempt>,
}

#[derive(Debug)]
pub struct Attempt {
    /// The URL, or `platform <name>`.
    pub source: String,
    pub error: Error,
}

/// Downloads the jar of `entry` into `<root>/mods/`, replacing the file there.
pub fn download(root: &Path, entry: &Entry, sources: Sources<'_>) -> Result<(), Failure> {
    let fail = |attempts| Failure {
        key: entry.key.clone(),
        attempts,
    };
    let dest = jar_path(root, entry).map_err(|error| {
        fail(vec![Attempt {
            source: "manifest".into(),
            error,
        }])
    })?;

    let mut attempts = Vec::new();
    let mut try_source =
        |source: String, open: &dyn Fn() -> Result<Box<dyn Read + Send>>| match open()
            .and_then(|mut reader| write_verified(&mut reader, &dest, entry))
        {
            Ok(()) => {
                tracing::info!(key = %entry.key, %source, "downloaded");
                true
            }
            Err(error) => {
                tracing::debug!(key = %entry.key, %source, "source failed");
                attempts.push(Attempt { source, error });
                false
            }
        };

    for url in &entry.downloads {
        if try_source(url.clone(), &|| sources.fetch.get(url)) {
            return Ok(());
        }
    }
    for platform in sources.registry.all() {
        if entry.platform(platform.name()).is_none() {
            continue;
        }
        let open = || match platform.download(entry)? {
            Download::Url(url) => sources.fetch.get(&url),
            Download::Stream(stream) => Ok(stream),
        };
        if try_source(format!("platform {}", platform.name()), &open) {
            return Ok(());
        }
    }
    if let Some(url) = &entry.url
        && try_source(url.clone(), &|| sources.fetch.get(url))
    {
        return Ok(());
    }

    if attempts.is_empty() && entry.is_local() {
        attempts.push(Attempt {
            source: "git".into(),
            error: Error::usage(
                format!(
                    "`{}` is a local mod and its jar {} is missing; git tracks it, nothing downloads it",
                    entry.key, entry.path
                ),
                format!(
                    "restore it with `git checkout -- {}`, or take the mod out with `modpack remove {}`",
                    entry.path, entry.key
                ),
            ),
        });
    }
    if attempts.is_empty() {
        attempts.push(Attempt {
            source: "manifest".into(),
            error: Error::usage(
                format!(
                    "`{}` has no download URL, no platform, no direct URL",
                    entry.key
                ),
                format!(
                    "put the jar in mods/ by hand as {}, or add the mod again from a platform",
                    entry.path
                ),
            ),
        });
    }
    Err(fail(attempts))
}

/// Downloads several jars, at most [`PARALLEL`] at a time, and records the
/// ones downloaded in [`Installed`]. The results are in the order of
/// `entries`.
pub fn download_all(
    root: &Path,
    entries: &[&Entry],
    sources: Sources<'_>,
) -> Result<Vec<Result<(), Failure>>> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(entries.len()));
    thread::scope(|scope| {
        for _ in 0..PARALLEL.min(entries.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(entry) = entries.get(i) else { break };
                    let result = download(root, entry, sources);
                    results.lock().expect("no worker panics").push((i, result));
                }
            });
        }
    });
    let mut results = results.into_inner().expect("no worker panics");
    results.sort_by_key(|(i, _)| *i);
    let done: Vec<&Entry> = results
        .iter()
        .filter(|(_, r)| r.is_ok())
        .map(|(i, _)| entries[*i])
        .collect();
    if !done.is_empty() {
        let mut installed = Installed::load(root)?;
        for entry in done {
            installed
                .jars
                .insert(entry.path.clone(), entry.hashes.sha1.clone());
        }
        installed.save(root)?;
    }
    Ok(results.into_iter().map(|(_, r)| r).collect())
}

fn write_verified(reader: &mut dyn Read, dest: &Path, entry: &Entry) -> Result<()> {
    write_atomic(dest, |file| {
        let received = copy_hashing(reader, file).context("receiving the jar")?;
        if !received.matches(&entry.hashes) {
            return Err(Error::internal(
                "the file received does not have the hashes of the manifest",
                "this source serves another file; if the mod changed on purpose, regenerate the manifest with `modpack manifest`",
            ));
        }
        Ok(())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::manifest::{Env, Hashes, Manifest, Pack};
    use crate::platform::fake::{FakeMod, FakePlatform};
    use crate::store::hash;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{self, Cursor};

    /// An entry `<key>` whose jar `mods/<key>-1.0.0.jar` holds `bytes`, with no
    /// source.
    pub(crate) fn entry(key: &str, bytes: &[u8]) -> Entry {
        let hashes = hash::copy_hashing(&mut &bytes[..], &mut io::sink()).unwrap();
        Entry {
            key: key.to_owned(),
            path: format!("mods/{key}-1.0.0.jar"),
            file_size: hashes.size,
            downloads: Vec::new(),
            url: None,
            source: None,
            approved: None,
            hashes: Hashes {
                sha1: hashes.sha1,
                sha512: hashes.sha512,
            },
            env: Env::default(),
            platforms: BTreeMap::new(),
        }
    }

    pub(crate) fn manifest(mods: Vec<Entry>) -> Manifest {
        let mut manifest = Manifest::new(Pack {
            minecraft: "1.21.1".into(),
            loader: "neoforge".into(),
            loader_version: "21.1.77".into(),
        });
        for entry in mods {
            manifest.upsert(entry);
        }
        manifest
    }

    /// Serves fixed bytes per URL; any other URL fails.
    #[derive(Default)]
    pub(crate) struct MapFetch(pub BTreeMap<String, Vec<u8>>);

    impl MapFetch {
        pub(crate) fn with(mut self, url: &str, bytes: &[u8]) -> Self {
            self.0.insert(url.to_owned(), bytes.to_vec());
            self
        }
    }

    impl Fetch for MapFetch {
        fn get(&self, url: &str) -> Result<Box<dyn Read + Send>> {
            match self.0.get(url) {
                Some(bytes) => Ok(Box::new(Cursor::new(bytes.clone()))),
                None => Err(Error::internal(format!("404 for {url}"), "none")),
            }
        }
    }

    /// Fails the test if anything is fetched.
    pub(crate) struct NoFetch;

    impl Fetch for NoFetch {
        fn get(&self, url: &str) -> Result<Box<dyn Read + Send>> {
            panic!("nothing should be fetched, got {url}")
        }
    }

    /// A platform hosting `sodium` with the given bytes.
    fn registry(bytes: &[u8]) -> Registry {
        let mut sodium = FakeMod::new("sodium");
        sodium.bytes = bytes.to_vec();
        Registry::new(vec![Box::new(FakePlatform::new("fake").with(sodium))])
    }

    /// `sodium` listing a download URL, a table on the fake platform and a
    /// direct URL.
    fn sodium() -> Entry {
        let mut e = entry("sodium", b"sodium");
        e.downloads = vec!["https://cdn.invalid/sodium.jar".into()];
        e.platform_mut("fake").insert("id".into(), "sodium".into());
        e.url = Some("https://direct.invalid/sodium.jar".into());
        e
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn jar(root: &Path) -> Vec<u8> {
        fs::read(root.join("mods/sodium-1.0.0.jar")).unwrap()
    }

    #[test]
    fn downloads_from_the_entry_url_first() {
        let root = tempfile::tempdir().unwrap();
        let fetch = MapFetch::default().with("https://cdn.invalid/sodium.jar", b"sodium");
        let sources = Sources {
            registry: &registry(b"never asked"),
            fetch: &fetch,
        };
        download(root.path(), &sodium(), sources).unwrap();
        assert_eq!(jar(root.path()), b"sodium");
        assert_eq!(files_in(&root.path().join("mods")), ["sodium-1.0.0.jar"]);
    }

    #[test]
    fn falls_back_to_the_platform() {
        let root = tempfile::tempdir().unwrap();
        let sources = Sources {
            registry: &registry(b"sodium"),
            fetch: &MapFetch::default(),
        };
        download(root.path(), &sodium(), sources).unwrap();
        assert_eq!(jar(root.path()), b"sodium");
    }

    #[test]
    fn wrong_hash_is_rejected_and_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let mut e = sodium();
        e.url = None;
        let fetch = MapFetch::default().with("https://cdn.invalid/sodium.jar", b"tampered");
        let sources = Sources {
            registry: &registry(b"also tampered"),
            fetch: &fetch,
        };
        let failure = download(root.path(), &e, sources).unwrap_err();
        assert_eq!(failure.attempts.len(), 2);
        assert!(
            failure
                .attempts
                .iter()
                .all(|a| a.error.to_string().contains("hashes of the manifest"))
        );
        assert!(files_in(&root.path().join("mods")).is_empty());
    }

    #[test]
    fn falls_back_to_the_direct_url() {
        let root = tempfile::tempdir().unwrap();
        let fetch = MapFetch::default().with("https://direct.invalid/sodium.jar", b"sodium");
        let sources = Sources {
            registry: &registry(b"tampered"),
            fetch: &fetch,
        };
        download(root.path(), &sodium(), sources).unwrap();
        assert_eq!(jar(root.path()), b"sodium");
    }

    #[test]
    fn every_source_failing_lists_every_attempt() {
        let root = tempfile::tempdir().unwrap();
        let sources = Sources {
            registry: &Registry::new(vec![Box::new(FakePlatform::new("fake"))]),
            fetch: &MapFetch::default(),
        };
        let failure = download(root.path(), &sodium(), sources).unwrap_err();
        assert_eq!(failure.key, "sodium");
        let tried: Vec<&str> = failure.attempts.iter().map(|a| a.source.as_str()).collect();
        assert_eq!(
            tried,
            [
                "https://cdn.invalid/sodium.jar",
                "platform fake",
                "https://direct.invalid/sodium.jar"
            ]
        );
        assert!(!root.path().join("mods/sodium-1.0.0.jar").exists());
    }

    #[test]
    fn platform_without_a_table_is_not_asked() {
        let root = tempfile::tempdir().unwrap();
        let mut e = sodium();
        e.platforms.clear();
        e.downloads.clear();
        e.url = None;
        let sources = Sources {
            registry: &registry(b"sodium"),
            fetch: &NoFetch,
        };
        let failure = download(root.path(), &e, sources).unwrap_err();
        assert_eq!(failure.attempts.len(), 1);
        assert!(failure.attempts[0].error.hint().contains("by hand"));
    }

    /// Sends a few bytes, then fails.
    struct Broken {
        sent: bool,
    }

    impl Read for Broken {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.sent {
                return Err(io::Error::other("connection reset"));
            }
            self.sent = true;
            buf[..3].copy_from_slice(b"sod");
            Ok(3)
        }
    }

    struct BrokenFetch;

    impl Fetch for BrokenFetch {
        fn get(&self, _: &str) -> Result<Box<dyn Read + Send>> {
            Ok(Box::new(Broken { sent: false }))
        }
    }

    #[test]
    fn interrupted_download_never_reaches_the_final_name() {
        let root = tempfile::tempdir().unwrap();
        let mods = root.path().join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("sodium-1.0.0.jar"), b"previous").unwrap();
        let mut e = sodium();
        e.platforms.clear();
        e.url = None;
        let sources = Sources {
            registry: &Registry::new(Vec::new()),
            fetch: &BrokenFetch,
        };
        let failure = download(root.path(), &e, sources).unwrap_err();
        assert!(
            failure.attempts[0]
                .error
                .to_string()
                .contains("connection reset")
        );
        assert_eq!(files_in(&mods), ["sodium-1.0.0.jar"]);
        assert_eq!(jar(root.path()), b"previous");
    }

    #[test]
    fn downloads_many_in_parallel_and_keeps_the_order() {
        let root = tempfile::tempdir().unwrap();
        let mut fetch = MapFetch::default();
        let entries: Vec<Entry> = (0..3 * PARALLEL)
            .map(|i| {
                let key = format!("mod{i}");
                let mut e = entry(&key, key.as_bytes());
                let url = format!("https://cdn.invalid/{key}.jar");
                // Every third mod is not served.
                if i % 3 != 0 {
                    fetch = std::mem::take(&mut fetch).with(&url, key.as_bytes());
                }
                e.downloads = vec![url];
                e
            })
            .collect();
        let refs: Vec<&Entry> = entries.iter().collect();
        let sources = Sources {
            registry: &Registry::new(Vec::new()),
            fetch: &fetch,
        };
        let results = download_all(root.path(), &refs, sources).unwrap();
        assert_eq!(results.len(), entries.len());
        let installed = Installed::load(root.path()).unwrap();
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.is_ok(), i % 3 != 0, "mod{i}");
            let e = &entries[i];
            assert_eq!(installed.placed(&e.path, &e.hashes.sha1), result.is_ok());
            if let Err(failure) = result {
                assert_eq!(failure.key, format!("mod{i}"));
            }
        }
        assert_eq!(files_in(&root.path().join("mods")).len(), 2 * PARALLEL);
    }
}
