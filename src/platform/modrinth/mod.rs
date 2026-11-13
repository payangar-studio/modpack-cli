//! The Modrinth platform, through its API v2. Reading needs no token.
//!
//! Its manifest table holds `project_id`, `version_id` and `dependencies`: the
//! project ids of the version's required dependencies, which the
//! `deps-complete` condition looks up among the other entries' `project_id`.

mod client;
mod mrpack;
mod page;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

use super::{
    Candidate, Capability, Download, Eligibility, Jar, Match, Operation, PackBuild, Platform,
    ProjectName,
};
use crate::error::{Error, Result};
use crate::model::manifest::{Entry, Env, Pack, Support};
use crate::secrets::{SecretName, Secrets};
use client::Client;

pub const NAME: &str = "modrinth";
pub const API_URL: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone)]
pub struct Modrinth {
    client: Client,
}

impl Default for Modrinth {
    fn default() -> Self {
        Self::new(API_URL)
    }
}

impl Modrinth {
    /// A client of the API at `api` (tests point it to a mock server).
    pub fn new(api: &str) -> Self {
        Self {
            client: Client::new(api),
        }
    }

    fn project(&self, id: &str) -> Result<Option<Project>> {
        self.client
            .get(&format!("/project/{}", encode_segment(id)), &[])
    }

    fn projects(&self, ids: &BTreeSet<&str>) -> Result<Vec<Project>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = serde_json::to_string(ids)?;
        Ok(self
            .client
            .get("/projects", &[("ids", &ids)])?
            .unwrap_or_default())
    }

    fn version(&self, id: &str) -> Result<Option<Version>> {
        self.client
            .get(&format!("/version/{}", encode_segment(id)), &[])
    }

    fn versions(&self, ids: &BTreeSet<&str>) -> Result<Vec<Version>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = serde_json::to_string(ids)?;
        Ok(self
            .client
            .get("/versions", &[("ids", &ids)])?
            .unwrap_or_default())
    }

    /// The version an entry's table points to.
    fn version_of(&self, entry: &Entry) -> Result<Version> {
        let id = entry
            .platform(NAME)
            .and_then(|t| t.get("version_id"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| not_on_modrinth(entry))?;
        self.version(id)?.ok_or_else(|| {
            Error::usage(
                format!("Modrinth no longer has version `{id}` of `{}`", entry.key),
                format!("add `{}` again to pick an available version", entry.key),
            )
        })
    }
}

impl Platform for Modrinth {
    fn name(&self) -> &str {
        NAME
    }

    fn capability(&self, _op: Operation) -> Capability {
        Capability::Api
    }

    fn project_name(&self, id: &str) -> Result<Option<ProjectName>> {
        Ok(self.project(id)?.map(|p| ProjectName {
            slug: p.slug,
            title: p.title,
        }))
    }

    fn identify(&self, jars: &[Jar]) -> Result<Vec<Option<Match>>> {
        if jars.is_empty() {
            return Ok(Vec::new());
        }
        let hashes: Vec<&str> = jars.iter().map(|j| j.sha512.as_str()).collect();
        let found: BTreeMap<String, Version> = self
            .client
            .post(
                "/version_files",
                &json!({ "hashes": hashes, "algorithm": "sha512" }),
            )?
            .unwrap_or_default();
        let project_ids = found.values().map(|v| v.project_id.as_str()).collect();
        let slugs: BTreeMap<String, String> = self
            .projects(&project_ids)?
            .into_iter()
            .map(|p| (p.id, p.slug))
            .collect();
        Ok(jars
            .iter()
            .map(|jar| {
                found.get(&jar.sha512).map(|v| Match {
                    slug: slugs.get(&v.project_id).cloned(),
                    section: section(&v.project_id, &v.id),
                    exact: true,
                })
            })
            .collect())
    }

    fn resolve(&self, query: &str, pack: &Pack) -> Result<Vec<Candidate>> {
        let Some(id) = parse_query(query) else {
            return Ok(Vec::new());
        };
        let Some(project) = self.project(&id)? else {
            return Ok(Vec::new());
        };
        let game_versions = serde_json::to_string(&[&pack.minecraft])?;
        let loaders = serde_json::to_string(&[&pack.loader])?;
        let mut versions: Vec<Version> = self
            .client
            .get(
                &format!("/project/{}/version", encode_segment(&project.id)),
                &[("game_versions", &game_versions), ("loaders", &loaders)],
            )?
            .unwrap_or_default();
        // Modrinth lists the newest first; releases go before betas and alphas.
        versions.sort_by_key(|v| match v.version_type.as_str() {
            "release" => 0,
            "beta" => 1,
            _ => 2,
        });
        Ok(versions
            .iter()
            .filter_map(|v| {
                let file = v.primary_file()?;
                Some(Candidate {
                    label: v.version_number.clone(),
                    slug: Some(project.slug.clone()),
                    file_name: file.filename.clone(),
                    section: section(&project.id, &v.id),
                })
            })
            .collect())
    }

    fn fill_entry(&self, entry: &mut Entry) -> Result<()> {
        let version = self.version_of(entry)?;
        let file = if entry.hashes.sha512.is_empty() {
            version.primary_file()
        } else {
            version
                .files
                .iter()
                .find(|f| f.hashes.sha512.eq_ignore_ascii_case(&entry.hashes.sha512))
        }
        .ok_or_else(|| {
            Error::usage(
                format!(
                    "the jar of `{}` is not a file of its Modrinth version `{}`",
                    entry.key, version.id
                ),
                format!("add `{}` again from Modrinth", entry.key),
            )
        })?;
        let project = self.project(&version.project_id)?.ok_or_else(|| {
            Error::internal(
                format!("Modrinth has no project `{}`", version.project_id),
                "retry later; if it persists, check https://status.modrinth.com",
            )
        })?;

        let required: Vec<&Dependency> = version
            .dependencies
            .iter()
            .filter(|d| d.dependency_type == "required")
            .collect();
        let unknown_projects: BTreeSet<&str> = required
            .iter()
            .filter(|d| d.project_id.is_none())
            .filter_map(|d| d.version_id.as_deref())
            .collect();
        let mut dependencies: BTreeSet<String> = required
            .iter()
            .filter_map(|d| d.project_id.clone())
            .collect();
        dependencies.extend(
            self.versions(&unknown_projects)?
                .into_iter()
                .map(|v| v.project_id),
        );
        dependencies.remove(&project.id);

        entry.downloads = vec![file.url.clone()];
        if entry.hashes.sha512.is_empty() {
            entry.hashes.sha1 = file.hashes.sha1.clone();
            entry.hashes.sha512 = file.hashes.sha512.clone();
            entry.file_size = file.size;
        }
        entry.env = Env {
            client: support(&project.client_side),
            server: support(&project.server_side),
        };
        let table = entry.platform_mut(NAME);
        table.insert("project_id".into(), project.id.into());
        table.insert("version_id".into(), version.id.into());
        table.insert(
            "dependencies".into(),
            toml::Value::Array(dependencies.into_iter().map(Into::into).collect()),
        );
        Ok(())
    }

    fn download(&self, entry: &Entry) -> Result<Download> {
        if entry.hashes.sha512.is_empty() {
            return Err(Error::internal(
                format!(
                    "`{}` has no sha512 to check its download against",
                    entry.key
                ),
                "regenerate the manifest with `modpack manifest`",
            ));
        }
        // The entry's `downloads` were already tried before the platforms:
        // ask the version for its current URL rather than the stored one.
        let url = self
            .version_of(entry)?
            .files
            .iter()
            .find(|f| f.hashes.sha512.eq_ignore_ascii_case(&entry.hashes.sha512))
            .map(|f| f.url.clone())
            .ok_or_else(|| not_on_modrinth(entry))?;
        Ok(Download::Stream(
            self.client.download(&url, &entry.hashes.sha512)?,
        ))
    }

    fn pack_eligibility(&self, entry: &Entry) -> Result<Eligibility> {
        Ok(if mrpack::allowed_downloads(entry).is_empty() {
            Eligibility::Blocked {
                reason: format!(
                    "no download URL from {}: the jar must be embedded in the pack, with its author's permission",
                    mrpack::ALLOWED_HOSTS.join(", ")
                ),
            }
        } else {
            Eligibility::Eligible
        })
    }

    fn build_pack(&self, build: &PackBuild<'_>) -> Result<PathBuf> {
        mrpack::build(build)
    }

    fn push_page(&self, root: &Path) -> Result<()> {
        let page = page::Page::load(root)?;
        let token = Secrets::system().get(SecretName::ModrinthToken)?;
        println!("{}", page::push(&self.client, &page, &token)?);
        Ok(())
    }
}

fn section(project_id: &str, version_id: &str) -> toml::Table {
    let mut section = toml::Table::new();
    section.insert("project_id".into(), project_id.into());
    section.insert("version_id".into(), version_id.into());
    section
}

fn not_on_modrinth(entry: &Entry) -> Error {
    Error::usage(
        format!("`{}` is not on Modrinth", entry.key),
        "get it from another platform, or give its direct URL",
    )
}

/// Modrinth's `client_side`/`server_side`. `unknown` (the author did not say)
/// is taken as required, so that the mod is never left out by mistake.
fn support(side: &str) -> Support {
    match side {
        "optional" => Support::Optional,
        "unsupported" => Support::Unsupported,
        _ => Support::Required,
    }
}

/// The project slug or id in `query`: the query itself, or the segment after
/// the project type in a `modrinth.com` URL (`https://modrinth.com/mod/sodium`).
/// `None` when the query is a URL of another site.
fn parse_query(query: &str) -> Option<String> {
    let query = query.trim();
    if !query.contains('/') {
        return (!query.is_empty()).then(|| query.to_owned());
    }
    let rest = query
        .strip_prefix("https://")
        .or_else(|| query.strip_prefix("http://"))
        .unwrap_or(query);
    let mut segments = rest.split(['/', '?', '#']).filter(|s| !s.is_empty());
    let host = segments.next()?.to_ascii_lowercase();
    if host != "modrinth.com" && host != "www.modrinth.com" {
        return None;
    }
    let _project_type = segments.next()?;
    segments.next().map(str::to_owned)
}

/// Percent-encodes a path segment, keeping RFC 3986 unreserved characters.
fn encode_segment(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct Project {
    id: String,
    slug: String,
    #[serde(default)]
    title: String,
    client_side: String,
    server_side: String,
}

#[derive(Debug, Deserialize)]
struct Version {
    id: String,
    project_id: String,
    version_number: String,
    #[serde(default)]
    version_type: String,
    files: Vec<VersionFile>,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

impl Version {
    /// The primary file, or the first one when none is marked primary.
    fn primary_file(&self) -> Option<&VersionFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.first())
    }
}

#[derive(Debug, Deserialize)]
struct VersionFile {
    hashes: FileHashes,
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct FileHashes {
    sha1: String,
    sha512: String,
}

#[derive(Debug, Deserialize)]
struct Dependency {
    version_id: Option<String>,
    project_id: Option<String>,
    dependency_type: String,
}
