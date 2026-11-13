//! The page push against a mock of the Modrinth API, and one test against
//! the real API (`#[ignore]`, run by hand).

use serde_json::{Value, json};
use tokio::runtime::Runtime;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::error::ErrorKind;
use crate::model::socle::InitValues;

const TOKEN: &str = "mrp_test-token-never-shown";
const ID: &str = "PrjAbc12";

/// A mock API served by a background runtime, while the blocking client runs
/// on the test thread.
struct Api {
    rt: Runtime,
    server: MockServer,
}

impl Api {
    fn start() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let server = rt.block_on(MockServer::start());
        Self { rt, server }
    }

    fn mount(&self, mock: Mock) {
        self.rt.block_on(mock.mount(&self.server));
    }

    fn client(&self) -> Client {
        Client::new(&self.server.uri())
    }

    fn requests(&self) -> Vec<Request> {
        self.rt.block_on(self.server.received_requests()).unwrap()
    }

    /// `METHOD /path?query` of every request received, in order.
    fn calls(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| {
                let query = r.url.query().map(|q| format!("?{q}")).unwrap_or_default();
                format!("{} {}{query}", r.method, r.url.path())
            })
            .collect()
    }

    /// Answers 204 to every write of the page: edit, icon, gallery.
    fn accept_writes(&self) {
        for (m, p) in [
            ("PATCH", format!("/project/{ID}")),
            ("PATCH", format!("/project/{ID}/icon")),
            ("POST", format!("/project/{ID}/gallery")),
            ("PATCH", format!("/project/{ID}/gallery")),
            ("DELETE", format!("/project/{ID}/gallery")),
        ] {
            self.mount(
                Mock::given(method(m))
                    .and(path(p))
                    .and(header("authorization", TOKEN))
                    .respond_with(ResponseTemplate::new(204)),
            );
        }
    }

    fn serve_project(&self, project: Value) {
        self.mount(
            Mock::given(method("GET"))
                .and(path(format!("/project/{ID}")))
                .and(header("authorization", TOKEN))
                .respond_with(ResponseTemplate::new(200).set_body_json(project)),
        );
    }
}

fn token() -> Secret {
    Secret::new(TOKEN)
}

fn sha1_of(bytes: &[u8]) -> String {
    hex(&Sha1::digest(bytes))
}

fn cdn(bytes: &[u8], suffix: &str) -> String {
    format!(
        "https://cdn.modrinth.com/data/{ID}/images/{}{suffix}",
        sha1_of(bytes)
    )
}

fn project(icon_url: Option<String>, gallery: Value) -> Value {
    json!({
        "id": ID,
        "slug": "test-pack",
        "title": "Test Pack",
        "icon_url": icon_url,
        "gallery": gallery,
    })
}

const PAGE_TOML: &str = r#"# The page of the pack.
name = "Test Pack"
summary = "A pack for tests."

[links]
source = "https://github.com/example/pack"
issues = ""

[modrinth]
# Written by `modpack page push`.
project_id = ""
categories = ["adventure"]
license = "MIT"

[curseforge]
project_id = ""
"#;

/// A repository with a page: `page.toml` as given, a description, an icon
/// and the gallery images given as (file, bytes).
fn repo(
    page_toml: &str,
    gallery_toml: Option<&str>,
    images: &[(&str, &[u8])],
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join(DIR);
    fs::create_dir_all(page.join(GALLERY_DIR)).unwrap();
    fs::write(page.join(super::PAGE_TOML), page_toml).unwrap();
    fs::write(page.join(DESCRIPTION), "# Test Pack\n\nLong text.\n").unwrap();
    fs::write(page.join(ICON), b"icon bytes").unwrap();
    if let Some(text) = gallery_toml {
        fs::write(page.join(GALLERY_DIR).join(GALLERY_TOML), text).unwrap();
    }
    for (file, bytes) in images {
        fs::write(page.join(GALLERY_DIR).join(file), bytes).unwrap();
    }
    dir
}

fn page_toml(root: &Path) -> String {
    fs::read_to_string(root.join(DIR).join(super::PAGE_TOML)).unwrap()
}

#[test]
fn creates_the_project_then_edits_it() {
    let gallery = r#"
[[image]]
file = "spawn.png"
title = "The spawn"
featured = true
"#;
    let dir = repo(PAGE_TOML, Some(gallery), &[("spawn.png", b"spawn bytes")]);
    let api = Api::start();
    api.mount(
        Mock::given(method("POST"))
            .and(path("/project"))
            .and(header("authorization", TOKEN))
            .respond_with(ResponseTemplate::new(200).set_body_json(project(None, json!([])))),
    );
    api.accept_writes();

    let page = Page::load(dir.path()).unwrap();
    let line = push(&api.client(), &page, &token()).unwrap();
    assert!(line.starts_with(&format!("modrinth: created draft project test-pack ({ID})")));
    assert!(line.contains("icon uploaded; gallery: 1 added, 0 updated, 0 removed"));
    assert_eq!(
        api.calls(),
        [
            "POST /project".to_owned(),
            format!("PATCH /project/{ID}/icon?ext=png"),
            format!(
                "POST /project/{ID}/gallery?featured=true&ordering=0&title=The%20spawn&ext=png"
            ),
        ]
    );

    // The creation is a multipart form whose `data` part holds the project.
    let create = &api.requests()[0];
    let content_type = create.headers["content-type"].to_str().unwrap().to_owned();
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let body = String::from_utf8(create.body.clone()).unwrap();
    let data = body
        .split(&format!("--{boundary}"))
        .find(|part| part.contains("name=\"data\""))
        .and_then(|part| part.split("\r\n\r\n").nth(1))
        .unwrap()
        .trim_end();
    let data: Value = serde_json::from_str(data).unwrap();
    assert_eq!(data["title"], "Test Pack");
    assert_eq!(data["slug"], "test-pack");
    assert_eq!(data["description"], "A pack for tests.");
    assert_eq!(data["body"], "# Test Pack\n\nLong text.\n");
    assert_eq!(data["project_type"], "modpack");
    assert_eq!(data["is_draft"], true);
    assert_eq!(data["initial_versions"], json!([]));
    assert_eq!(data["categories"], json!(["adventure"]));
    assert_eq!(data["license_id"], "MIT");
    assert_eq!(data["source_url"], "https://github.com/example/pack");
    assert_eq!(data["issues_url"], Value::Null);

    // The id is recorded, the rest of page.toml kept as written.
    let written = page_toml(dir.path());
    assert_eq!(
        written,
        PAGE_TOML.replacen("project_id = \"\"", &format!("project_id = \"{ID}\""), 1)
    );

    // Second push: the project exists, its icon and gallery match the page.
    let api = Api::start();
    api.serve_project(project(
        Some(cdn(b"icon bytes", "_96.webp")),
        json!([{
            "url": cdn(b"spawn bytes", ".png"),
            "featured": true,
            "title": "The spawn",
            "description": null,
            "created": "2026-01-01T00:00:00Z",
            "ordering": 0,
        }]),
    ));
    api.mount(
        Mock::given(method("PATCH"))
            .and(path(format!("/project/{ID}")))
            .and(body_partial_json(json!({
                "title": "Test Pack",
                "description": "A pack for tests.",
                "license_id": "MIT",
                "issues_url": null,
            })))
            .respond_with(ResponseTemplate::new(204)),
    );
    let page = Page::load(dir.path()).unwrap();
    let line = push(&api.client(), &page, &token()).unwrap();
    assert_eq!(
        line,
        format!(
            "modrinth: updated project test-pack ({ID}); gallery: 0 added, 0 updated, 0 removed"
        )
    );
    assert_eq!(
        api.calls(),
        [format!("GET /project/{ID}"), format!("PATCH /project/{ID}")]
    );
}

#[test]
fn gallery_follows_the_page_and_icon_is_replaced() {
    let with_id = PAGE_TOML.replacen("project_id = \"\"", &format!("project_id = \"{ID}\""), 1);
    let gallery = r#"
[[image]]
file = "b.png"
title = "Second, first"

[[image]]
file = "a.png"
title = "Kept"
"#;
    let dir = repo(
        &with_id,
        Some(gallery),
        &[("a.png", b"a"), ("b.png", b"b"), ("c.jpg", b"c")],
    );
    fs::write(
        dir.path().join(DIR).join(GALLERY_DIR).join("notes.txt"),
        "x",
    )
    .unwrap();
    let api = Api::start();
    api.serve_project(project(
        Some(cdn(b"old icon", ".png")),
        json!([
            { "url": cdn(b"a", ".png"), "featured": false, "title": "Kept", "description": null, "created": "x", "ordering": 0 },
            { "url": cdn(b"gone", "_350.webp"), "featured": false, "title": null, "description": null, "created": "x", "ordering": 1 },
        ]),
    ));
    api.accept_writes();

    let page = Page::load(dir.path()).unwrap();
    let line = push(&api.client(), &page, &token()).unwrap();
    assert!(line.ends_with("icon uploaded; gallery: 2 added, 1 updated, 1 removed"));
    let gone = cdn(b"gone", "_350.webp")
        .replace(':', "%3A")
        .replace('/', "%2F");
    let a = cdn(b"a", ".png").replace(':', "%3A").replace('/', "%2F");
    assert_eq!(
        api.calls(),
        [
            format!("GET /project/{ID}"),
            format!("PATCH /project/{ID}"),
            format!("PATCH /project/{ID}/icon?ext=png"),
            format!("DELETE /project/{ID}/gallery?url={gone}"),
            format!(
                "POST /project/{ID}/gallery?featured=false&ordering=0&title=Second%2C%20first&ext=png"
            ),
            format!("PATCH /project/{ID}/gallery?featured=false&ordering=1&title=Kept&url={a}"),
            format!("POST /project/{ID}/gallery?featured=false&ordering=2&ext=jpg"),
        ]
    );
    let upload = &api.requests()[4];
    assert_eq!(upload.body, b"b");
    assert_eq!(upload.headers["content-type"], "image/png");
    assert_eq!(api.requests()[6].headers["content-type"], "image/jpeg");
}

#[test]
fn creation_needs_a_license_and_sends_nothing_without_it() {
    let dir = repo(&PAGE_TOML.replace("license = \"MIT\"", ""), None, &[]);
    let api = Api::start();
    let page = Page::load(dir.path()).unwrap();
    let err = push(&api.client(), &page, &token()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("modrinth.license"));
    assert!(api.calls().is_empty());
}

#[test]
fn unknown_project_id_is_a_usage_error() {
    let with_id = PAGE_TOML.replacen("project_id = \"\"", "project_id = \"nope\"", 1);
    let dir = repo(&with_id, None, &[]);
    let api = Api::start();
    let page = Page::load(dir.path()).unwrap();
    let err = push(&api.client(), &page, &token()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("Modrinth has no project `nope`"));
    assert!(err.hint().contains("empty it to create a new project"));
}

#[test]
fn refusal_carries_the_reason_given_by_modrinth() {
    let dir = repo(PAGE_TOML, None, &[]);
    let api = Api::start();
    api.mount(
        Mock::given(method("POST"))
            .and(path("/project"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": "invalid_input",
                "description": "Invalid category: adventure",
            }))),
    );
    let page = Page::load(dir.path()).unwrap();
    let err = push(&api.client(), &page, &token()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("Invalid category: adventure"));
    assert!(!err.to_string().contains(TOKEN));
    // Nothing was created, so nothing is recorded.
    assert_eq!(page_toml(dir.path()), PAGE_TOML);
}

#[test]
fn refused_token_points_to_auth() {
    let with_id = PAGE_TOML.replacen("project_id = \"\"", &format!("project_id = \"{ID}\""), 1);
    let dir = repo(&with_id, None, &[]);
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path(format!("/project/{ID}")))
            .respond_with(ResponseTemplate::new(401)),
    );
    let page = Page::load(dir.path()).unwrap();
    let err = push(&api.client(), &page, &token()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.hint().contains("modpack auth modrinth-token"));
}

#[test]
fn gallery_lists_its_images_first_then_the_others() {
    let gallery = r#"
[[image]]
file = "z.png"
description = "Last by name, first by list"
"#;
    let dir = repo(
        PAGE_TOML,
        Some(gallery),
        &[("b.webp", b"b"), ("z.png", b"z"), ("a.PNG", b"a")],
    );
    let page = Page::load(dir.path()).unwrap();
    let files: Vec<_> = page.gallery.iter().map(|g| g.image.file.as_str()).collect();
    assert_eq!(files, ["z.png", "a.PNG", "b.webp"]);
    assert_eq!(
        page.gallery[0].description.as_deref(),
        Some("Last by name, first by list")
    );
    assert_eq!(page.gallery[1].image.ext, "png");
}

#[test]
fn gallery_errors_name_the_problem() {
    let missing = "[[image]]\nfile = \"missing.png\"\n";
    let err = Page::load(repo(PAGE_TOML, Some(missing), &[]).path()).unwrap_err();
    assert!(err.to_string().contains("lists `missing.png`"));

    let two = "[[image]]\nfile = \"a.png\"\nfeatured = true\n[[image]]\nfile = \"b.png\"\nfeatured = true\n";
    let err = Page::load(repo(PAGE_TOML, Some(two), &[("a.png", b"a"), ("b.png", b"b")]).path())
        .unwrap_err();
    assert!(err.to_string().contains("more than one image"));

    let typo = "[[image]]\nfile = \"a.png\"\ntitel = \"x\"\n";
    let err = Page::load(repo(PAGE_TOML, Some(typo), &[("a.png", b"a")]).path()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("gallery.toml is invalid"));
}

/// The skeleton `modpack init` lays down: the pack name comes from init, and
/// the summary is asked for before anything is sent.
#[test]
fn page_template_needs_a_summary_and_takes_the_pack_name() {
    let dir = tempfile::tempdir().unwrap();
    crate::socle::init(
        dir.path(),
        InitValues {
            name: "Skeleton Pack".into(),
            minecraft: "1.21.1".into(),
            loader: "neoforge".into(),
            loader_version: "21.1.77".into(),
            author: "Tester".into(),
        },
    )
    .unwrap();
    let err = Page::load(dir.path()).unwrap_err();
    assert!(err.to_string().contains("has no summary"));

    let path = dir.path().join(DIR).join(super::PAGE_TOML);
    let text =
        fs::read_to_string(&path)
            .unwrap()
            .replacen("summary = \"\"", "summary = \"Filled.\"", 1);
    fs::write(&path, text).unwrap();
    let page = Page::load(dir.path()).unwrap();
    assert_eq!(page.name, "Skeleton Pack");
    assert!(page.icon.is_none());
    assert!(page.gallery.is_empty());
    assert!(page.modrinth.project_id.is_empty());
}

#[test]
fn url_sha1_reads_raw_and_resized_names() {
    assert_eq!(
        url_sha1("https://cdn.modrinth.com/data/X/images/abc123.png"),
        "abc123"
    );
    assert_eq!(
        url_sha1("https://cdn.modrinth.com/data/X/abc123_96.webp"),
        "abc123"
    );
}

#[test]
fn slug_comes_from_the_name() {
    assert_eq!(slugify("Test Pack"), "test-pack");
    assert_eq!(slugify("  Créa: Mods & More! "), "cr-a-mods-more");
}

#[test]
fn multipart_boundary_never_appears_in_the_data() {
    let json = "{\"body\":\"modpack-cli-form-boundary\"}";
    let (content_type, body) = multipart_data(json);
    assert_eq!(
        content_type,
        "multipart/form-data; boundary=modpack-cli-form-boundaryx"
    );
    let body = String::from_utf8(body).unwrap();
    assert!(body.starts_with("--modpack-cli-form-boundaryx\r\n"));
    assert!(body.ends_with("\r\n--modpack-cli-form-boundaryx--\r\n"));
}

/// A valid 1x1 PNG: Modrinth decodes the images it receives.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// Against the real API, on an existing test project the token can edit:
/// `MODPACK_MODRINTH_TOKEN` and `MODPACK_TEST_MODRINTH_PROJECT` (its id).
/// Pushes a page with one gallery image, then the same page without it.
/// Never creates a project, so that running it leaves nothing behind.
#[test]
#[ignore = "edits a real Modrinth test project; run by hand"]
fn real_api_edits_a_test_project() {
    let (Ok(token), Ok(id)) = (
        std::env::var("MODPACK_MODRINTH_TOKEN"),
        std::env::var("MODPACK_TEST_MODRINTH_PROJECT"),
    ) else {
        eprintln!("skipped: set MODPACK_MODRINTH_TOKEN and MODPACK_TEST_MODRINTH_PROJECT");
        return;
    };
    let toml = format!(
        "name = \"modpack-cli test\"\nsummary = \"Test project of modpack-cli, edited by its tests.\"\n\n[modrinth]\nproject_id = \"{id}\"\n"
    );
    let dir = repo(&toml, None, &[("shot.png", PNG)]);
    fs::write(dir.path().join(DIR).join(ICON), PNG).unwrap();
    let client = Client::new(super::super::API_URL);
    let token = Secret::new(token);
    let api = super::Api {
        client: &client,
        token: &token,
    };

    let line = push(&client, &Page::load(dir.path()).unwrap(), &token).unwrap();
    println!("{line}");
    let remote = api.project(&id).unwrap();
    assert!(
        remote
            .gallery
            .iter()
            .any(|r| url_sha1(&r.url).eq_ignore_ascii_case(&sha1_of(PNG)))
    );
    assert!(remote.icon_url.is_some());

    fs::remove_file(dir.path().join(DIR).join(GALLERY_DIR).join("shot.png")).unwrap();
    let line = push(&client, &Page::load(dir.path()).unwrap(), &token).unwrap();
    println!("{line}");
    assert!(api.project(&id).unwrap().gallery.is_empty());
}
