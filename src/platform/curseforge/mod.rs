//! CurseForge, through its Core API (`https://api.curseforge.com`).
//!
//! Its manifest table holds `projectID` and `fileID`, the names a CurseForge
//! pack's `manifest.json` uses, and once filled `dependencies`: the project
//! ids of the file's required dependencies.
//!
//! Every network operation needs the Core API key. Download URLs are never
//! written to the manifest: the API terms forbid storing them, and they are
//! read again at each download.

mod api;
mod fingerprint;
mod pack;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use api::{Api, File};
use tracing::warn;

use super::{
    Candidate, Capability, Download, Eligibility, Jar, Match, Operation, PackBuild, Platform,
    ProjectName,
};
use crate::error::{Context, Error, Result};
use crate::model::manifest::{Entry, Pack};
use crate::secrets::{SecretName, Secrets};

pub use fingerprint::fingerprint;

/// Name of the platform and of its manifest table.
pub const NAME: &str = "curseforge";

const BASE_URL: &str = "https://api.curseforge.com";
const KEY_VAR: &str = "MODPACK_CURSEFORGE_API_KEY";

/// The Core API key, if one is set: the environment, then the system keyring
/// where `modpack auth` stores it.
pub fn api_key() -> Option<String> {
    match Secrets::system().find(SecretName::CurseforgeApiKey) {
        Ok(found) => found
            .map(|(key, _)| key.expose().to_owned())
            .filter(|k| !k.trim().is_empty()),
        Err(err) => {
            warn!("{err}");
            None
        }
    }
}

pub struct CurseForge {
    base_url: String,
    key: fn() -> Option<String>,
    /// Whether `key` gives one, read once: packing asks for every mod.
    has_key: OnceLock<bool>,
}

impl CurseForge {
    pub fn new() -> Self {
        Self::with(BASE_URL, api_key)
    }

    /// A client of the API at `base_url`, taking its key from `key`.
    pub fn with(base_url: &str, key: fn() -> Option<String>) -> Self {
        Self {
            base_url: base_url.to_owned(),
            key,
            has_key: OnceLock::new(),
        }
    }

    fn api(&self) -> Result<Api> {
        let key = (self.key)().ok_or_else(|| {
            Error::usage(
                "CurseForge needs a Core API key, and none is set",
                format!("store it with `modpack auth`, or set {KEY_VAR}"),
            )
        })?;
        Ok(Api::new(&self.base_url, key))
    }
}

impl Default for CurseForge {
    fn default() -> Self {
        Self::new()
    }
}

/// `ModLoaderType` of a loader named as in the manifest.
fn loader(name: &str) -> Result<u8> {
    match name {
        "forge" => Ok(1),
        "fabric" => Ok(4),
        "quilt" => Ok(5),
        "neoforge" => Ok(6),
        _ => Err(Error::usage(
            format!("CurseForge does not know the loader `{name}`"),
            "use one of: neoforge, fabric, forge, quilt",
        )),
    }
}

/// Project and file ids from an entry's table.
fn ids(entry: &Entry) -> Result<(u64, u64)> {
    let get = |field: &str| {
        entry
            .platform(NAME)
            .and_then(|t| t.get(field))
            .and_then(toml::Value::as_integer)
            .and_then(|v| u64::try_from(v).ok())
    };
    match (get("projectID"), get("fileID")) {
        (Some(project), Some(file)) => Ok((project, file)),
        _ => Err(Error::usage(
            format!("`{}` has no CurseForge project and file ids", entry.key),
            "add the mod from CurseForge, or leave CurseForge out (`--platform`)",
        )),
    }
}

fn section(project_id: u64, file_id: u64) -> toml::Table {
    let mut section = toml::Table::new();
    section.insert("projectID".into(), int(project_id));
    section.insert("fileID".into(), int(file_id));
    section
}

fn int(v: u64) -> toml::Value {
    toml::Value::Integer(i64::try_from(v).expect("CurseForge ids fit in an i64"))
}

/// What the user typed to name a mod.
#[derive(Debug, PartialEq, Eq)]
enum Query {
    Id(u64),
    Slug(String),
}

/// Reads a slug, a project id, or a CurseForge URL
/// (`https://www.curseforge.com/minecraft/mc-mods/<slug>[/files/<file id>]`),
/// with the file id the URL names. `None` for a URL of another site.
fn parse_query(query: &str) -> Option<(Query, Option<u64>)> {
    let query = query.trim();
    let rest = query
        .strip_prefix("https://")
        .or_else(|| query.strip_prefix("http://"));
    let Some(rest) = rest else {
        return Some(match query.parse() {
            Ok(id) => (Query::Id(id), None),
            Err(_) => (Query::Slug(query.to_owned()), None),
        });
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host != "curseforge.com" && !host.ends_with(".curseforge.com") {
        return None;
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let after = |name: &str| {
        parts
            .iter()
            .position(|p| *p == name)
            .and_then(|i| parts.get(i + 1))
            .copied()
    };
    let file = after("files").and_then(|f| f.parse().ok());
    let project = match (after("mc-mods"), after("projects")) {
        (Some(slug), _) => Query::Slug(slug.to_owned()),
        (None, Some(id)) => Query::Id(id.parse().ok()?),
        (None, None) => return None,
    };
    Some((project, file))
}

fn label(file: &File) -> String {
    let release = match file.release_type {
        2 => "beta",
        3 => "alpha",
        _ => "release",
    };
    let date = file.file_date.get(..10).unwrap_or(&file.file_date);
    format!("{} ({release}, {date})", file.display_name)
}

/// Whether a file declares the pack's Minecraft version and loader.
fn fits(file: &File, pack: &Pack) -> bool {
    let has = |name: &str| {
        file.game_versions
            .iter()
            .any(|v| v.eq_ignore_ascii_case(name))
    };
    has(&pack.minecraft) && has(&pack.loader)
}

impl Platform for CurseForge {
    fn name(&self) -> &str {
        NAME
    }

    fn capability(&self, op: Operation) -> Capability {
        match op {
            Operation::PushPage => Capability::Skill("modpack-curseforge-page"),
            _ => Capability::Api,
        }
    }

    fn project_name(&self, id: &str) -> Result<Option<ProjectName>> {
        let Ok(id) = id.parse() else {
            return Ok(None);
        };
        Ok(self.api()?.get_mod(id)?.map(|m| ProjectName {
            slug: m.slug,
            title: m.name,
        }))
    }

    fn identify(&self, jars: &[Jar]) -> Result<Vec<Option<Match>>> {
        if jars.is_empty() {
            return Ok(Vec::new());
        }
        let api = self.api()?;
        let prints = jars
            .iter()
            .map(|jar| {
                fs::read(&jar.path)
                    .map(|bytes| fingerprint(&bytes))
                    .context(format!("reading {}", jar.path.display()))
            })
            .collect::<Result<Vec<u32>>>()?;
        let found = api.fingerprints(&prints)?;

        // Fingerprint sent -> (project, file, exact).
        let mut matched: BTreeMap<u64, (u64, u64, bool)> = BTreeMap::new();
        for m in &found.exact_matches {
            matched.insert(m.file.file_fingerprint, (m.id, m.file.id, true));
        }
        for m in &found.partial_matches {
            let keys = [m.id, m.file.id, m.file.file_fingerprint].map(|k| k.to_string());
            let sent = keys
                .iter()
                .find_map(|k| found.partial_match_fingerprints.get(k));
            let Some(sent) = sent else {
                warn!(
                    project = m.id,
                    file = m.file.id,
                    "CurseForge returned a partial match no jar can be tied to; ignored"
                );
                continue;
            };
            for print in sent {
                matched.entry(*print).or_insert((m.id, m.file.id, false));
            }
        }

        let projects: BTreeSet<u64> = matched.values().map(|(p, _, _)| *p).collect();
        let slugs: BTreeMap<u64, String> = if projects.is_empty() {
            BTreeMap::new()
        } else {
            let ids: Vec<u64> = projects.into_iter().collect();
            api.mods(&ids)?
                .into_iter()
                .map(|m| (m.id, m.slug))
                .collect()
        };
        Ok(prints
            .iter()
            .map(|print| {
                matched
                    .get(&u64::from(*print))
                    .map(|&(project, file, exact)| Match {
                        slug: slugs.get(&project).cloned(),
                        section: section(project, file),
                        exact,
                    })
            })
            .collect())
    }

    fn resolve(&self, query: &str, pack: &Pack) -> Result<Vec<Candidate>> {
        let loader = loader(&pack.loader)?;
        let Some((project, file_id)) = parse_query(query) else {
            return Ok(Vec::new());
        };
        let api = self.api()?;
        let found = match project {
            Query::Id(id) => api.get_mod(id)?,
            Query::Slug(slug) => api.mod_by_slug(&slug)?,
        };
        let Some(m) = found else {
            return Ok(Vec::new());
        };
        let mut files = match file_id {
            Some(file_id) => api
                .file(m.id, file_id)?
                .filter(|f| fits(f, pack))
                .into_iter()
                .collect(),
            None => api.files(m.id, &pack.minecraft, loader)?,
        };
        files.sort_by(|a, b| b.file_date.cmp(&a.file_date));
        Ok(files
            .into_iter()
            .map(|f| Candidate {
                label: label(&f),
                slug: Some(m.slug.clone()),
                section: section(f.mod_id, f.id),
                file_name: f.file_name,
            })
            .collect())
    }

    fn fill_entry(&self, entry: &mut Entry) -> Result<()> {
        let (project, file_id) = ids(entry)?;
        let file = self
            .api()?
            .file(project, file_id)?
            .ok_or_else(|| gone(entry))?;
        let dependencies: BTreeSet<u64> = file
            .dependencies
            .iter()
            .filter(|d| d.relation_type == api::REQUIRED_DEPENDENCY)
            .map(|d| d.mod_id)
            .collect();
        entry.platform_mut(NAME).insert(
            "dependencies".into(),
            toml::Value::Array(dependencies.into_iter().map(int).collect()),
        );
        Ok(())
    }

    fn download(&self, entry: &Entry) -> Result<Download> {
        let (project, file_id) = ids(entry)?;
        let api = self.api()?;
        let file = api.file(project, file_id)?.ok_or_else(|| gone(entry))?;
        if let Some(url) = file.download_url.filter(|u| !u.is_empty()) {
            return Ok(Download::Url(url));
        }
        let page = api
            .get_mod(project)
            .ok()
            .flatten()
            .and_then(|m| m.links.website_url)
            .unwrap_or_else(|| format!("https://www.curseforge.com/projects/{project}"));
        Err(Error::usage(
            format!(
                "`{}` cannot be downloaded again automatically: its author does not allow downloads outside CurseForge",
                entry.key
            ),
            format!(
                "download {} by hand from {page} and put it in mods/, or add the mod from a direct URL",
                file.file_name
            ),
        ))
    }

    fn pack_eligibility(&self, entry: &Entry) -> Result<Eligibility> {
        if ids(entry).is_ok() {
            return Ok(Eligibility::Eligible);
        }
        let reason = if *self.has_key.get_or_init(|| (self.key)().is_some()) {
            "not hosted on CurseForge: a CurseForge pack accepts it only if it is on CurseForge's approved list of third-party mods, which has no API; check it by hand".to_owned()
        } else {
            format!(
                "no CurseForge ids in mods.lock.toml, and no CurseForge Core API key to look them up: store one with `modpack auth` or set {KEY_VAR}, then run `modpack manifest` to fill them"
            )
        };
        Ok(Eligibility::Blocked { reason })
    }

    fn embedded_jar_note(&self) -> Option<String> {
        Some("a CurseForge pack accepts an embedded jar only if it is on CurseForge's approved list of third-party mods, which has no API; check it by hand".to_owned())
    }

    fn build_pack(&self, build: &PackBuild<'_>) -> Result<PathBuf> {
        pack::build(build)
    }
}

/// The file an entry points to no longer exists on CurseForge.
fn gone(entry: &Entry) -> Error {
    Error::usage(
        format!("the CurseForge file of `{}` no longer exists", entry.key),
        format!("add `{}` again to pick another file", entry.key),
    )
}

#[cfg(test)]
mod tests;
