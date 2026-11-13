//! `push_page` of Modrinth: pushes the page versioned in `page/` to the
//! project, by the API.
//!
//! Without a project id in `page/page.toml`, the project is created as a
//! draft modpack and its id is written back to `page.toml`. With one, the
//! project's texts, links, categories and license are edited. In both cases
//! the icon and the gallery are then brought in line with `page/`: the
//! repository is the source of truth, so an image removed from `page/gallery/`
//! is removed from Modrinth.
//!
//! Modrinth names every uploaded image after the sha1 of its bytes
//! (`<sha1>.<ext>`, or `<sha1>_<width>.webp` once resized), which is how a
//! local image is matched to a remote one without recording anything else.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use ureq::http::{Response, StatusCode};
use ureq::typestate::WithBody;
use ureq::{Body, RequestBuilder};

use super::client::Client;
use super::{NAME, encode_segment};
use crate::error::{Context, Error, Result};
use crate::model::socle::SocleConfig;
use crate::secrets::Secret;

/// Directory of the page, relative to the repository root.
pub const DIR: &str = "page";
const PAGE_TOML: &str = "page.toml";
const DESCRIPTION: &str = "description.md";
const ICON: &str = "icon.png";
const GALLERY_DIR: &str = "gallery";
const GALLERY_TOML: &str = "gallery.toml";
/// Extensions Modrinth accepts for images, with their media type.
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
];
/// Sides declared at creation, required by the API. A modpack is played on
/// the client; a server may or may not run it.
const CLIENT_SIDE: &str = "required";
const SERVER_SIDE: &str = "optional";

/// The page as versioned in `page/`, with what Modrinth needs of it.
#[derive(Debug)]
pub struct Page {
    root: PathBuf,
    name: String,
    summary: String,
    body: String,
    links: Links,
    modrinth: Section,
    icon: Option<Image>,
    gallery: Vec<GalleryImage>,
}

/// `page/page.toml`. Other platforms' tables are left to them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PageToml {
    name: String,
    summary: String,
    links: Links,
    modrinth: Section,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Links {
    source: String,
    issues: String,
    wiki: String,
    discord: String,
}

/// The `[modrinth]` table of `page.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Section {
    project_id: String,
    slug: String,
    categories: Vec<String>,
    additional_categories: Vec<String>,
    license: String,
}

/// `page/gallery/gallery.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct GalleryToml {
    image: Vec<GalleryEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GalleryEntry {
    file: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    featured: bool,
}

#[derive(Debug)]
struct Image {
    file: String,
    ext: String,
    media_type: &'static str,
    bytes: Vec<u8>,
    sha1: String,
}

#[derive(Debug)]
struct GalleryImage {
    image: Image,
    title: Option<String>,
    description: Option<String>,
    featured: bool,
}

impl Page {
    /// Reads `page/` under the repository `root`. An empty `name` falls back
    /// to the pack name given to `modpack init`.
    pub fn load(root: &Path) -> Result<Self> {
        let dir = root.join(DIR);
        let toml_path = dir.join(PAGE_TOML);
        let text = fs::read_to_string(&toml_path).map_err(|err| {
            Error::usage(
                format!("cannot read {DIR}/{PAGE_TOML}: {err}"),
                format!(
                    "create {DIR}/ with {PAGE_TOML} and {DESCRIPTION}; `modpack init` lays them down in a new repository"
                ),
            )
        })?;
        let toml: PageToml = toml::from_str(&text).map_err(|err| {
            Error::usage(
                format!("{DIR}/{PAGE_TOML} is invalid: {}", err.message()),
                format!("fix {DIR}/{PAGE_TOML}"),
            )
        })?;
        let mut name = toml.name.trim().to_owned();
        if name.is_empty() {
            name = SocleConfig::load(root)?.init.name;
        }
        let summary = toml.summary.trim().to_owned();
        if summary.is_empty() {
            return Err(Error::usage(
                format!("{DIR}/{PAGE_TOML} has no summary"),
                format!("set `summary` in {DIR}/{PAGE_TOML}: one or two sentences"),
            ));
        }
        let body = fs::read_to_string(dir.join(DESCRIPTION)).map_err(|err| {
            Error::usage(
                format!("cannot read {DIR}/{DESCRIPTION}: {err}"),
                format!("write the long description of the pack in {DIR}/{DESCRIPTION}"),
            )
        })?;
        let icon_path = dir.join(ICON);
        let icon = icon_path
            .is_file()
            .then(|| Image::read(&icon_path))
            .transpose()?;
        Ok(Self {
            root: root.to_path_buf(),
            name,
            summary,
            body,
            links: toml.links,
            modrinth: toml.modrinth,
            icon,
            gallery: read_gallery(&dir.join(GALLERY_DIR))?,
        })
    }
}

impl Image {
    fn read(path: &Path) -> Result<Self> {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (ext, media_type) = image_type(path).ok_or_else(|| {
            Error::usage(
                format!("{file} is not an image Modrinth accepts"),
                format!(
                    "use one of these extensions: {}",
                    IMAGE_TYPES
                        .iter()
                        .map(|(e, _)| *e)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
        })?;
        let bytes = fs::read(path).context(format!("reading {}", path.display()))?;
        let sha1 = hex(&Sha1::digest(&bytes));
        Ok(Self {
            file,
            ext,
            media_type,
            bytes,
            sha1,
        })
    }
}

fn image_type(path: &Path) -> Option<(String, &'static str)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let media_type = IMAGE_TYPES.iter().find(|(e, _)| *e == ext)?.1;
    Some((ext, media_type))
}

/// The gallery in display order: the images `gallery.toml` lists, in its
/// order, then the other images of the directory by file name.
fn read_gallery(dir: &Path) -> Result<Vec<GalleryImage>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let toml_path = dir.join(GALLERY_TOML);
    let listed: GalleryToml = if toml_path.is_file() {
        let text = fs::read_to_string(&toml_path)
            .context(format!("reading {DIR}/{GALLERY_DIR}/{GALLERY_TOML}"))?;
        toml::from_str(&text).map_err(|err| {
            Error::usage(
                format!(
                    "{DIR}/{GALLERY_DIR}/{GALLERY_TOML} is invalid: {}",
                    err.message()
                ),
                format!("fix {DIR}/{GALLERY_DIR}/{GALLERY_TOML}"),
            )
        })?
    } else {
        GalleryToml::default()
    };

    let mut gallery = Vec::new();
    for entry in listed.image {
        let path = dir.join(&entry.file);
        if !path.is_file() {
            return Err(Error::usage(
                format!(
                    "{DIR}/{GALLERY_DIR}/{GALLERY_TOML} lists `{}`, which is not in {DIR}/{GALLERY_DIR}/",
                    entry.file
                ),
                "add the image, or remove its entry",
            ));
        }
        gallery.push(GalleryImage {
            image: Image::read(&path)?,
            title: non_empty(entry.title),
            description: non_empty(entry.description),
            featured: entry.featured,
        });
    }
    let mut others: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    others.retain(|p| p.is_file() && image_type(p).is_some());
    others.sort();
    for path in others {
        let image = Image::read(&path)?;
        if gallery.iter().all(|g| g.image.file != image.file) {
            gallery.push(GalleryImage {
                image,
                title: None,
                description: None,
                featured: false,
            });
        }
    }
    if gallery.iter().filter(|g| g.featured).count() > 1 {
        return Err(Error::usage(
            format!("{DIR}/{GALLERY_DIR}/{GALLERY_TOML} features more than one image"),
            "keep `featured = true` on one image only",
        ));
    }
    Ok(gallery)
}

fn non_empty(text: String) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The project as Modrinth returns it, reduced to what the push compares.
#[derive(Debug, Deserialize)]
struct Remote {
    id: String,
    slug: Option<String>,
    icon_url: Option<String>,
    #[serde(default)]
    gallery: Vec<RemoteImage>,
}

#[derive(Debug, Deserialize)]
struct RemoteImage {
    url: String,
    featured: bool,
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    ordering: i64,
}

/// The sha1 Modrinth named an uploaded image after, from its URL.
fn url_sha1(url: &str) -> &str {
    let name = url.rsplit('/').next().unwrap_or(url);
    name.split(['.', '_']).next().unwrap_or(name)
}

/// What a push changed, for the line printed at the end.
#[derive(Debug, Default, PartialEq, Eq)]
struct Changes {
    icon: bool,
    added: usize,
    updated: usize,
    removed: usize,
}

/// Pushes `page` to Modrinth with `token`, and returns a line saying what
/// was done.
pub fn push(client: &Client, page: &Page, token: &Secret) -> Result<String> {
    let api = Api { client, token };
    let section = &page.modrinth;
    let (remote, created) = if section.project_id.trim().is_empty() {
        let remote = api.create(page)?;
        record_project_id(&page.root, &remote.id)?;
        (remote, true)
    } else {
        let id = section.project_id.trim();
        let remote = api.project(id)?;
        api.edit(&remote.id, page)?;
        (remote, false)
    };
    let mut changes = Changes {
        icon: sync_icon(&api, &remote, page.icon.as_ref())?,
        ..Changes::default()
    };
    sync_gallery(&api, &remote, &page.gallery, &mut changes)?;

    let slug = remote.slug.as_deref().unwrap_or(&remote.id);
    let mut line = if created {
        format!(
            "{NAME}: created draft project {slug} ({}), id recorded in {DIR}/{PAGE_TOML}; submit it for review on Modrinth once the page is ready",
            remote.id
        )
    } else {
        format!("{NAME}: updated project {slug} ({})", remote.id)
    };
    if changes.icon {
        line.push_str("; icon uploaded");
    }
    line.push_str(&format!(
        "; gallery: {} added, {} updated, {} removed",
        changes.added, changes.updated, changes.removed
    ));
    Ok(line)
}

/// Writes the project id into the `[modrinth]` table of `page.toml`,
/// keeping the rest of the file as it is.
fn record_project_id(root: &Path, id: &str) -> Result<()> {
    let path = root.join(DIR).join(PAGE_TOML);
    let text = fs::read_to_string(&path).context(format!("reading {DIR}/{PAGE_TOML}"))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().context(format!("parsing {DIR}/{PAGE_TOML}"))?;
    if !doc.contains_table(NAME) {
        doc[NAME] = toml_edit::table();
    }
    doc[NAME]["project_id"] = toml_edit::value(id);
    fs::write(&path, doc.to_string()).context(format!("writing {DIR}/{PAGE_TOML}"))
}

fn sync_icon(api: &Api, remote: &Remote, icon: Option<&Image>) -> Result<bool> {
    let Some(icon) = icon else {
        return Ok(false);
    };
    let current = remote.icon_url.as_deref().map(url_sha1);
    if current.is_some_and(|sha1| sha1.eq_ignore_ascii_case(&icon.sha1)) {
        return Ok(false);
    }
    api.write(
        Method::Patch,
        &format!("/project/{}/icon", encode_segment(&remote.id)),
        &[("ext", icon.ext.clone())],
        Some(icon),
    )?;
    Ok(true)
}

fn sync_gallery(
    api: &Api,
    remote: &Remote,
    gallery: &[GalleryImage],
    changes: &mut Changes,
) -> Result<()> {
    let path = format!("/project/{}/gallery", encode_segment(&remote.id));
    let local = |r: &RemoteImage| {
        gallery
            .iter()
            .any(|g| url_sha1(&r.url).eq_ignore_ascii_case(&g.image.sha1))
    };
    for stale in remote.gallery.iter().filter(|r| !local(r)) {
        api.write(Method::Delete, &path, &[("url", stale.url.clone())], None)?;
        changes.removed += 1;
    }
    for (ordering, wanted) in gallery.iter().enumerate() {
        let ordering = ordering as i64;
        let mut query = vec![
            ("featured", wanted.featured.to_string()),
            ("ordering", ordering.to_string()),
        ];
        query.extend(wanted.title.clone().map(|t| ("title", t)));
        query.extend(wanted.description.clone().map(|d| ("description", d)));
        let existing = remote
            .gallery
            .iter()
            .find(|r| url_sha1(&r.url).eq_ignore_ascii_case(&wanted.image.sha1));
        match existing {
            None => {
                query.push(("ext", wanted.image.ext.clone()));
                api.write(Method::Post, &path, &query, Some(&wanted.image))?;
                changes.added += 1;
            }
            Some(r) => {
                // An empty local title or description leaves the remote one:
                // the API cannot clear them.
                let same = r.featured == wanted.featured
                    && r.ordering == ordering
                    && wanted
                        .title
                        .as_ref()
                        .is_none_or(|t| r.title.as_ref() == Some(t))
                    && wanted
                        .description
                        .as_ref()
                        .is_none_or(|d| r.description.as_ref() == Some(d));
                if !same {
                    query.push(("url", r.url.clone()));
                    api.write(Method::Patch, &path, &query, None)?;
                    changes.updated += 1;
                }
            }
        }
    }
    Ok(())
}

/// Authenticated calls of the page push.
struct Api<'a> {
    client: &'a Client,
    token: &'a Secret,
}

impl Api<'_> {
    /// The project, which must exist (a draft is only visible with the token).
    fn project(&self, id: &str) -> Result<Remote> {
        let path = format!("/project/{}", encode_segment(id));
        let response = self.client.call(&format!("GET {path}"), |agent, api| {
            agent
                .get(format!("{api}{path}"))
                .header("Authorization", self.token.expose())
                .call()
        })?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(Error::usage(
                format!("Modrinth has no project `{id}`, or the token cannot see it"),
                format!(
                    "check `{NAME}.project_id` in {DIR}/{PAGE_TOML}; empty it to create a new project"
                ),
            ));
        }
        read(response, &path)
    }

    /// Creates the project as a draft modpack.
    fn create(&self, page: &Page) -> Result<Remote> {
        let section = &page.modrinth;
        if section.license.trim().is_empty() {
            return Err(Error::usage(
                format!(
                    "{DIR}/{PAGE_TOML} has no `{NAME}.license`, required to create the project"
                ),
                format!(
                    "set `{NAME}.license` to an SPDX id, such as `MIT` or `LicenseRef-All-Rights-Reserved`"
                ),
            ));
        }
        let slug = match section.slug.trim() {
            "" => slugify(&page.name),
            slug => slug.to_owned(),
        };
        let mut data = fields(page);
        data["slug"] = json!(slug);
        data["project_type"] = json!("modpack");
        data["client_side"] = json!(CLIENT_SIDE);
        data["server_side"] = json!(SERVER_SIDE);
        data["initial_versions"] = json!([]);
        data["is_draft"] = json!(true);
        let (content_type, body) = multipart_data(&data.to_string());
        let response = self.client.call("POST /project", |agent, api| {
            agent
                .post(format!("{api}/project"))
                .header("Authorization", self.token.expose())
                .header("Content-Type", &content_type)
                .send(&body[..])
        })?;
        read(response, "/project")
    }

    /// Edits the texts, links, categories and license of the project.
    fn edit(&self, id: &str, page: &Page) -> Result<()> {
        let path = format!("/project/{}", encode_segment(id));
        let data = fields(page);
        let response = self.client.call(&format!("PATCH {path}"), |agent, api| {
            agent
                .patch(format!("{api}{path}"))
                .header("Authorization", self.token.expose())
                .send_json(&data)
        })?;
        check(response, &path).map(drop)
    }

    /// A write without JSON body: `POST` or `PATCH` with the image as raw
    /// body or with nothing, `DELETE`; the parameters go in the query.
    fn write(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        image: Option<&Image>,
    ) -> Result<()> {
        let pairs = query.iter().map(|(k, v)| (*k, v.as_str()));
        let response = self
            .client
            .call(&format!("{method} {path}"), |agent, api| {
                let url = format!("{api}{path}");
                let with_body = |request: RequestBuilder<WithBody>| {
                    let request = request
                        .header("Authorization", self.token.expose())
                        .query_pairs(pairs.clone());
                    match image {
                        Some(image) => request
                            .header("Content-Type", image.media_type)
                            .send(&image.bytes[..]),
                        None => request.send_empty(),
                    }
                };
                match method {
                    Method::Post => with_body(agent.post(&url)),
                    Method::Patch => with_body(agent.patch(&url)),
                    Method::Delete => agent
                        .delete(&url)
                        .header("Authorization", self.token.expose())
                        .query_pairs(pairs.clone())
                        .call(),
                }
            })?;
        check(response, path).map(drop)
    }
}

#[derive(Debug, Clone, Copy)]
enum Method {
    Post,
    Patch,
    Delete,
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Method::Post => "POST",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        })
    }
}

/// The fields create and edit share. An empty link is sent as `null`, which
/// clears it.
fn fields(page: &Page) -> Value {
    let link = |url: &str| match url.trim() {
        "" => Value::Null,
        url => json!(url),
    };
    let section = &page.modrinth;
    let mut data = json!({
        "title": page.name,
        "description": page.summary,
        "body": page.body,
        "categories": section.categories,
        "additional_categories": section.additional_categories,
        "source_url": link(&page.links.source),
        "issues_url": link(&page.links.issues),
        "wiki_url": link(&page.links.wiki),
        "discord_url": link(&page.links.discord),
    });
    if !section.license.trim().is_empty() {
        data["license_id"] = json!(section.license.trim());
    }
    data
}

/// A slug from the pack name: lowercase ASCII letters and digits, other
/// characters turned into single dashes.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_owned()
}

/// A `multipart/form-data` body with the one `data` part project creation
/// needs, and its content type.
fn multipart_data(json: &str) -> (String, Vec<u8>) {
    let mut boundary = String::from("modpack-cli-form-boundary");
    while json.contains(&boundary) {
        boundary.push('x');
    }
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"data\"\r\nContent-Type: application/json\r\n\r\n{json}\r\n--{boundary}--\r\n"
    );
    (
        format!("multipart/form-data; boundary={boundary}"),
        body.into_bytes(),
    )
}

/// The response, or an error saying what Modrinth refused and why.
fn check(mut response: Response<Body>, path: &str) -> Result<Response<Body>> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // Modrinth explains a refusal in `{"error": ..., "description": ...}`.
    let reason = response
        .body_mut()
        .read_json::<Value>()
        .ok()
        .and_then(|v| v.get("description")?.as_str().map(str::to_owned))
        .map(|d| format!(": {d}"))
        .unwrap_or_default();
    let what = format!("Modrinth answered {status} to {path}{reason}");
    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Error::usage(
            what,
            "check the Modrinth token and its scopes (PROJECT_CREATE, PROJECT_WRITE), then store it with `modpack auth modrinth-token`",
        ),
        StatusCode::BAD_REQUEST => Error::usage(what, format!("fix {DIR}/ accordingly")),
        _ => Error::internal(
            what,
            "retry later; if it persists, check https://status.modrinth.com",
        ),
    })
}

fn read(response: Response<Body>, path: &str) -> Result<Remote> {
    check(response, path)?
        .body_mut()
        .read_json()
        .context(format!("reading the answer of Modrinth to {path}"))
}

#[cfg(test)]
mod tests;
