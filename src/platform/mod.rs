//! The contract every platform implements, and the registry the core asks.
//!
//! The core never names a platform: it iterates over the registry, or looks a
//! platform up by a name it read somewhere (a manifest table, `--platform`).
//! Adding a platform is adding an implementation and one line in
//! [`Registry::builtin`].
//!
//! Each platform owns the table named after it in every manifest entry
//! ([`Entry::platforms`]); nobody else reads or writes it.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::manifest::{Entry, Pack};

pub mod curseforge;
#[cfg(test)]
pub mod fake;
pub mod modrinth;

/// The operations of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    Identify,
    Resolve,
    FillEntry,
    Download,
    PackEligibility,
    BuildPack,
    PushPage,
}

impl Operation {
    pub const ALL: [Operation; 7] = [
        Operation::Identify,
        Operation::Resolve,
        Operation::FillEntry,
        Operation::Download,
        Operation::PackEligibility,
        Operation::BuildPack,
        Operation::PushPage,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Operation::Identify => "identify",
            Operation::Resolve => "resolve",
            Operation::FillEntry => "fill_entry",
            Operation::Download => "download",
            Operation::PackEligibility => "pack_eligibility",
            Operation::BuildPack => "build_pack",
            Operation::PushPage => "push_page",
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How a platform carries out an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// By code calling the platform's API.
    Api,
    /// By the named skill, which an agent runs itself (in a browser for
    /// instance). The CLI only tells which skill to run.
    Skill(&'static str),
    Unsupported,
}

/// A jar to identify, with what the core computed from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jar {
    pub path: PathBuf,
    pub sha1: String,
    pub sha512: String,
    pub size: u64,
}

/// A jar a platform recognized.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// The project's slug on the platform, used to build the manifest key.
    pub slug: Option<String>,
    /// The platform's table for the manifest entry.
    pub section: toml::Table,
    /// False when the platform recognized the jar only partly (a CurseForge
    /// fingerprint close to, but not equal to, a known file): never to be
    /// taken as an exact match without asking.
    pub exact: bool,
}

/// A version of a mod the user asked for, compatible with the pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// What to show when asking which version to take.
    pub label: String,
    pub slug: Option<String>,
    /// File name of the jar, used as its name in `mods/`.
    pub file_name: String,
    /// The platform's table for the manifest entry.
    pub section: toml::Table,
}

/// How a project shows to people: its slug and its title.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectName {
    pub slug: String,
    pub title: String,
}

/// Where to get a jar from.
pub enum Download {
    Url(String),
    Stream(Box<dyn Read + Send>),
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Download::Url(url) => f.debug_tuple("Url").field(url).finish(),
            Download::Stream(_) => f.write_str("Stream(..)"),
        }
    }
}

/// Whether a mod can go into this platform's pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    /// Blocks the pack for this platform, for the reason given.
    Blocked {
        reason: String,
    },
}

/// What a pack artifact is built from.
#[derive(Debug, Clone)]
pub struct PackBuild<'a> {
    /// Name of the pack.
    pub name: &'a str,
    /// Version of the pack being built.
    pub version: &'a str,
    pub pack: &'a Pack,
    /// The mods of the mode.
    pub mods: Vec<&'a Entry>,
    /// Files to put under `overrides/`, in order: a later one replaces an
    /// earlier one at the same path.
    pub overrides: Vec<Override>,
    /// Directory to write the artifact in.
    pub out_dir: &'a Path,
    /// File name of the artifact without extension; the platform adds its own.
    pub file_stem: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    /// File on disk.
    pub source: PathBuf,
    /// Path inside `overrides/`, with `/` separators.
    pub path: String,
}

/// The contract of a platform. Operations a platform declares `Unsupported`
/// keep the default body, which fails with a clear error.
pub trait Platform: Send + Sync {
    /// Name of the platform, also the name of its manifest table.
    fn name(&self) -> &str;

    fn capability(&self, op: Operation) -> Capability;

    /// Recognizes jars by their hashes or fingerprint. The result has one item
    /// per jar, in order; `None` for a jar the platform does not know.
    fn identify(&self, jars: &[Jar]) -> Result<Vec<Option<Match>>> {
        let _ = jars;
        Err(unsupported(self.name(), Operation::Identify))
    }

    /// Versions of the mod `query` (slug, id or URL) compatible with `pack`,
    /// best first. Empty when the platform does not know the mod.
    fn resolve(&self, query: &str, pack: &Pack) -> Result<Vec<Candidate>> {
        let _ = (query, pack);
        Err(unsupported(self.name(), Operation::Resolve))
    }

    /// Slug and title of the project with the id `id`, to name it to people
    /// rather than by its id. `None` when the platform does not know it or
    /// cannot say.
    fn project_name(&self, id: &str) -> Result<Option<ProjectName>> {
        let _ = id;
        Ok(None)
    }

    /// Completes an entry from its table for this platform: download URLs,
    /// sides, required dependencies (kept in the platform table).
    fn fill_entry(&self, entry: &mut Entry) -> Result<()> {
        let _ = entry;
        Err(unsupported(self.name(), Operation::FillEntry))
    }

    /// Where to get the entry's jar.
    fn download(&self, entry: &Entry) -> Result<Download> {
        let _ = entry;
        Err(unsupported(self.name(), Operation::Download))
    }

    fn pack_eligibility(&self, entry: &Entry) -> Result<Eligibility> {
        let _ = entry;
        Err(unsupported(self.name(), Operation::PackEligibility))
    }

    /// Builds the pack artifact and returns its path.
    fn build_pack(&self, build: &PackBuild<'_>) -> Result<PathBuf> {
        let _ = build;
        Err(unsupported(self.name(), Operation::BuildPack))
    }

    /// Pushes the page versioned in the repository at `root` to the platform.
    fn push_page(&self, root: &Path) -> Result<()> {
        let _ = root;
        Err(unsupported(self.name(), Operation::PushPage))
    }
}

/// Error of an operation a platform does not carry out by code.
pub fn unsupported(platform: &str, op: Operation) -> Error {
    Error::usage(
        format!("platform `{platform}` does not support `{op}`"),
        "leave this platform out of the command (`--platform`)",
    )
}

/// The known platforms.
pub struct Registry {
    platforms: Vec<Box<dyn Platform>>,
}

impl Registry {
    pub fn new(platforms: Vec<Box<dyn Platform>>) -> Self {
        Self { platforms }
    }

    /// The platforms built into the CLI, in the order they are tried.
    pub fn builtin() -> Self {
        Self::new(vec![
            Box::new(modrinth::Modrinth::default()),
            Box::new(curseforge::CurseForge::new()),
        ])
    }

    pub fn all(&self) -> impl Iterator<Item = &dyn Platform> {
        self.platforms.iter().map(Box::as_ref)
    }

    pub fn names(&self) -> Vec<&str> {
        self.all().map(Platform::name).collect()
    }

    pub fn find(&self, name: &str) -> Option<&dyn Platform> {
        self.all().find(|p| p.name() == name)
    }

    /// Like [`Registry::find`], for a name the user typed.
    pub fn get(&self, name: &str) -> Result<&dyn Platform> {
        self.find(name).ok_or_else(|| {
            Error::usage(
                format!("unknown platform `{name}`"),
                format!("use one of: {}", self.names().join(", ")),
            )
        })
    }

    /// Every operation of a platform with its capability.
    pub fn capabilities(platform: &dyn Platform) -> Vec<(Operation, Capability)> {
        Operation::ALL
            .into_iter()
            .map(|op| (op, platform.capability(op)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakePlatform;
    use super::*;
    use crate::error::ErrorKind;

    fn registry() -> Registry {
        Registry::new(vec![
            Box::new(FakePlatform::new("fake")),
            Box::new(FakePlatform::new("other")),
        ])
    }

    #[test]
    fn registry_finds_a_platform_by_name() {
        let registry = registry();
        assert_eq!(registry.names(), ["fake", "other"]);
        assert_eq!(registry.find("other").unwrap().name(), "other");
        assert!(registry.find("nope").is_none());
    }

    #[test]
    fn unknown_platform_lists_the_known_ones() {
        let err = registry().get("nope").err().unwrap();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert_eq!(err.hint(), "use one of: fake, other");
    }

    #[test]
    fn registry_exposes_capabilities() {
        let registry = registry();
        let caps = Registry::capabilities(registry.find("fake").unwrap());
        assert_eq!(caps.len(), Operation::ALL.len());
        assert!(caps.contains(&(Operation::Identify, Capability::Api)));
        assert!(caps.contains(&(Operation::PushPage, Capability::Skill("modpack-fake-page"))));
    }

    /// A platform that implements nothing.
    struct Bare;

    impl Platform for Bare {
        fn name(&self) -> &str {
            "bare"
        }
        fn capability(&self, _: Operation) -> Capability {
            Capability::Unsupported
        }
    }

    #[test]
    fn unimplemented_operations_fail_with_a_clear_error() {
        let err = Bare.push_page(Path::new(".")).unwrap_err();
        assert_eq!(
            err.to_string().lines().next().unwrap(),
            "error: platform `bare` does not support `push_page`"
        );
        assert!(Bare.identify(&[]).is_err());
    }
}
