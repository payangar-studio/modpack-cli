//! Client of the CurseForge Core API (`https://api.curseforge.com`), and the
//! parts of its JSON the platform reads.
//!
//! Every call needs the Core API key, sent in the `x-api-key` header. The key
//! never appears in a message: errors name the request, not its headers.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// Game id of Minecraft on CurseForge.
pub const MINECRAFT: u32 = 432;
/// Class id of mods, among Minecraft's classes (mods, modpacks, resource packs...).
pub const CLASS_MODS: u32 = 6;
/// `FileRelationType` of a required dependency.
pub const REQUIRED_DEPENDENCY: u8 = 3;

const USER_AGENT: &str = concat!("payangar-studio/modpack-cli/", env!("CARGO_PKG_VERSION"));

pub struct Api {
    base: String,
    key: String,
    agent: ureq::Agent,
}

#[derive(Debug, Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mod {
    pub id: u64,
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub links: Links,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Links {
    pub website_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u64,
    pub mod_id: u64,
    pub display_name: String,
    pub file_name: String,
    /// 1 release, 2 beta, 3 alpha.
    #[serde(default)]
    pub release_type: u8,
    /// ISO 8601, so that text order is date order.
    #[serde(default)]
    pub file_date: String,
    /// Null when the author does not allow third-party downloads. According
    /// to the API terms, it must not be stored.
    pub download_url: Option<String>,
    /// Minecraft versions, loaders (`NeoForge`, `Fabric`...) and sides mixed.
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub file_fingerprint: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub mod_id: u64,
    pub relation_type: u8,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintMatches {
    #[serde(default)]
    pub exact_matches: Vec<FingerprintMatch>,
    #[serde(default)]
    pub partial_matches: Vec<FingerprintMatch>,
    /// Fingerprints sent that matched only partly. The API documents neither
    /// the key nor the value; clients read the key as a project id and the
    /// value as the fingerprints sent.
    #[serde(default)]
    pub partial_match_fingerprints: HashMap<String, Vec<u64>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FingerprintMatch {
    /// Project id.
    pub id: u64,
    pub file: File,
}

impl Api {
    pub fn new(base: &str, key: String) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self {
            base: base.trim_end_matches('/').to_owned(),
            key,
            agent,
        }
    }

    pub fn fingerprints(&self, fingerprints: &[u32]) -> Result<FingerprintMatches> {
        let body = serde_json::json!({ "fingerprints": fingerprints });
        let what = "looking jars up by fingerprint";
        self.post(&format!("/v1/fingerprints/{MINECRAFT}"), &body, what)?
            .ok_or_else(|| unexpected(what, 404))
    }

    /// The projects with these ids; unknown ids are left out.
    pub fn mods(&self, ids: &[u64]) -> Result<Vec<Mod>> {
        let body = serde_json::json!({ "modIds": ids });
        Ok(self
            .post("/v1/mods", &body, "reading projects")?
            .unwrap_or_default())
    }

    pub fn get_mod(&self, id: u64) -> Result<Option<Mod>> {
        self.get(&format!("/v1/mods/{id}"), &[], "reading a project")
    }

    /// The mod whose slug is `slug`, if any.
    pub fn mod_by_slug(&self, slug: &str) -> Result<Option<Mod>> {
        let query = [
            ("gameId", MINECRAFT.to_string()),
            ("classId", CLASS_MODS.to_string()),
            ("slug", slug.to_owned()),
        ];
        let found: Vec<Mod> = self
            .get("/v1/mods/search", &query, "searching a project")?
            .unwrap_or_default();
        Ok(found.into_iter().find(|m| m.slug == slug))
    }

    /// The latest files of a mod for a Minecraft version and a loader
    /// (`ModLoaderType`), one page of 50 at most.
    pub fn files(&self, mod_id: u64, minecraft: &str, loader: u8) -> Result<Vec<File>> {
        let query = [
            ("gameVersion", minecraft.to_owned()),
            ("modLoaderType", loader.to_string()),
            ("pageSize", "50".to_owned()),
        ];
        Ok(self
            .get(&format!("/v1/mods/{mod_id}/files"), &query, "listing files")?
            .unwrap_or_default())
    }

    pub fn file(&self, mod_id: u64, file_id: u64) -> Result<Option<File>> {
        self.get(
            &format!("/v1/mods/{mod_id}/files/{file_id}"),
            &[],
            "reading a file",
        )
    }

    /// `None` when the API answers 404.
    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        what: &str,
    ) -> Result<Option<T>> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("x-api-key", &self.key)
            .header("accept", "application/json");
        for (k, v) in query {
            request = request.query(k, v);
        }
        read(request.call(), what)
    }

    fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
        what: &str,
    ) -> Result<Option<T>> {
        let response = self
            .agent
            .post(format!("{}{path}", self.base))
            .header("x-api-key", &self.key)
            .header("accept", "application/json")
            .send_json(body);
        read(response, what)
    }
}

fn read<T: DeserializeOwned>(
    response: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    what: &str,
) -> Result<Option<T>> {
    let mut response = response.map_err(|e| {
        Error::internal(
            format!("could not reach CurseForge while {what}"),
            "check the network connection, then retry",
        )
        .with_source(e)
    })?;
    match response.status().as_u16() {
        200..=299 => {}
        404 => return Ok(None),
        401 | 403 => {
            return Err(Error::usage(
                format!("CurseForge refused the API key while {what}"),
                "check the Core API key: store a valid one with `modpack auth`, or set MODPACK_CURSEFORGE_API_KEY",
            ));
        }
        status => return Err(unexpected(what, status)),
    }
    let data: Data<T> = response.body_mut().read_json().map_err(|e| {
        Error::internal(
            format!("CurseForge sent an unexpected answer while {what}"),
            "retry later; if it keeps failing, report it at https://github.com/payangar-studio/modpack-cli/issues",
        )
        .with_source(e)
    })?;
    Ok(Some(data.data))
}

fn unexpected(what: &str, status: u16) -> Error {
    Error::internal(
        format!("CurseForge answered HTTP {status} while {what}"),
        "retry later; CurseForge may be down",
    )
}
