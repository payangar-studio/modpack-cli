//! Tests against a local mock of the API, and `#[ignore]` ones against the
//! real API, which skip themselves without a key.

use std::path::Path;

use mockito::{Matcher, Server};
use serde_json::json;

use super::*;
use crate::error::ErrorKind;
use crate::model::manifest::{Env, Hashes};

const KEY: &str = "test-key";

fn with_key(server: &Server) -> CurseForge {
    CurseForge::with(&server.url(), || Some(KEY.into()))
}

fn pack() -> Pack {
    Pack {
        minecraft: "1.21.1".into(),
        loader: "neoforge".into(),
        loader_version: "21.1.77".into(),
    }
}

fn file(id: u64, mod_id: u64, date: &str) -> serde_json::Value {
    json!({
        "id": id,
        "modId": mod_id,
        "displayName": format!("Mod {id}"),
        "fileName": format!("mod-{id}.jar"),
        "releaseType": 1,
        "fileDate": date,
        "downloadUrl": format!("https://edge.forgecdn.net/files/{id}/mod-{id}.jar"),
        "gameVersions": ["1.21.1", "NeoForge", "Client", "Server"],
        "dependencies": [
            { "modId": 7, "relationType": 3 },
            { "modId": 8, "relationType": 2 },
            { "modId": 6, "relationType": 3 }
        ],
        "fileFingerprint": 1,
        "unknownField": true
    })
}

fn entry(project: u64, file: u64) -> Entry {
    Entry {
        key: "jei".into(),
        path: "mods/jei.jar".into(),
        file_size: 1,
        downloads: Vec::new(),
        url: None,
        hashes: Hashes {
            sha1: String::new(),
            sha512: String::new(),
        },
        env: Env::default(),
        platforms: [(NAME.to_owned(), section(project, file))].into(),
    }
}

fn jar(path: &Path) -> Jar {
    Jar {
        path: path.to_owned(),
        sha1: String::new(),
        sha512: String::new(),
        size: 0,
    }
}

fn fixture_jar() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/curseforge/fixture-1.0.0.jar")
}

#[test]
fn every_network_operation_asks_for_the_key_when_missing() {
    let server = Server::new();
    let cf = CurseForge::with(&server.url(), || None);
    let errors = [
        cf.identify(&[jar(&fixture_jar())]).unwrap_err(),
        cf.resolve("jei", &pack()).unwrap_err(),
        cf.fill_entry(&mut entry(1, 2)).unwrap_err(),
        cf.download(&entry(1, 2)).unwrap_err(),
    ];
    for err in errors {
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.hint().contains("modpack auth"), "{err}");
        assert!(err.hint().contains(KEY_VAR), "{err}");
    }
}

#[test]
fn operations_without_network_need_no_key() {
    let cf = CurseForge::with("http://unused.invalid", || None);
    assert!(cf.identify(&[]).unwrap().is_empty());
    assert!(
        cf.resolve("https://modrinth.com/mod/sodium", &pack())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        cf.pack_eligibility(&entry(1, 2)).unwrap(),
        Eligibility::Eligible
    );
}

#[test]
fn declares_its_capabilities() {
    let cf = CurseForge::new();
    assert_eq!(cf.name(), "curseforge");
    assert_eq!(cf.capability(Operation::Identify), Capability::Api);
    assert_eq!(
        cf.capability(Operation::PushPage),
        Capability::Skill("modpack-curseforge-page")
    );
    assert!(crate::platform::Registry::builtin().find(NAME).is_some());
}

#[test]
fn identifies_exact_partial_and_unknown_jars() {
    let mut server = Server::new();
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("partial.jar");
    let unknown = dir.path().join("unknown.jar");
    fs::write(&partial, b"partial").unwrap();
    fs::write(&unknown, b"unknown").unwrap();
    let exact_print = 550_359_374u64;
    let partial_print = u64::from(fingerprint(b"partial"));
    let unknown_print = u64::from(fingerprint(b"unknown"));

    let mut exact_file = file(5846877, 238222, "2024-09-01T00:00:00Z");
    exact_file["fileFingerprint"] = json!(exact_print);
    let mut partial_file = file(4000000, 306612, "2024-01-01T00:00:00Z");
    partial_file["fileFingerprint"] = json!(42);
    let prints = server
        .mock("POST", "/v1/fingerprints/432")
        .match_header("x-api-key", KEY)
        .match_body(Matcher::Json(json!({
            "fingerprints": [exact_print, partial_print, unknown_print]
        })))
        .with_body(
            json!({ "data": {
                "isCacheBuilt": true,
                "exactMatches": [{ "id": 238222, "file": exact_file, "latestFiles": [] }],
                "exactFingerprints": [exact_print],
                "partialMatches": [{ "id": 306612, "file": partial_file, "latestFiles": [] }],
                "partialMatchFingerprints": { "306612": [partial_print] },
                "installedFingerprints": [],
                "unmatchedFingerprints": [unknown_print]
            }})
            .to_string(),
        )
        .create();
    let mods = server
        .mock("POST", "/v1/mods")
        .match_body(Matcher::Json(json!({ "modIds": [238222, 306612] })))
        .with_body(
            json!({ "data": [
                { "id": 238222, "slug": "jei" },
                { "id": 306612, "slug": "fabric-api" }
            ]})
            .to_string(),
        )
        .create();

    let found = with_key(&server)
        .identify(&[jar(&fixture_jar()), jar(&partial), jar(&unknown)])
        .unwrap();
    prints.assert();
    mods.assert();

    let exact = found[0].as_ref().unwrap();
    assert!(exact.exact);
    assert_eq!(exact.slug.as_deref(), Some("jei"));
    assert_eq!(exact.section, section(238222, 5846877));
    let partial = found[1].as_ref().unwrap();
    assert!(!partial.exact, "a partial match is never exact");
    assert_eq!(partial.section, section(306612, 4000000));
    assert!(found[2].is_none());
}

#[test]
fn resolves_a_slug_to_compatible_files_newest_first() {
    let mut server = Server::new();
    let search = server
        .mock("GET", "/v1/mods/search")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("gameId".into(), "432".into()),
            Matcher::UrlEncoded("classId".into(), "6".into()),
            Matcher::UrlEncoded("slug".into(), "jei".into()),
        ]))
        .with_body(json!({ "data": [{ "id": 238222, "slug": "jei" }] }).to_string())
        .create();
    let files = server
        .mock("GET", "/v1/mods/238222/files")
        .match_header("x-api-key", KEY)
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("gameVersion".into(), "1.21.1".into()),
            Matcher::UrlEncoded("modLoaderType".into(), "6".into()),
        ]))
        .with_body(
            json!({ "data": [
                file(1, 238222, "2024-01-01T00:00:00Z"),
                file(2, 238222, "2024-06-01T00:00:00Z")
            ], "pagination": { "index": 0, "pageSize": 50, "resultCount": 2, "totalCount": 2 }})
            .to_string(),
        )
        .create();

    let candidates = with_key(&server).resolve("jei", &pack()).unwrap();
    search.assert();
    files.assert();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].section, section(238222, 2));
    assert_eq!(candidates[0].file_name, "mod-2.jar");
    assert_eq!(candidates[0].slug.as_deref(), Some("jei"));
    assert_eq!(candidates[0].label, "Mod 2 (release, 2024-06-01)");
}

#[test]
fn project_name_gives_slug_and_title() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/238222")
        .with_body(
            json!({ "data": { "id": 238222, "slug": "jei", "name": "Just Enough Items" } })
                .to_string(),
        )
        .create();
    let platform = with_key(&server);
    assert_eq!(
        platform.project_name("238222").unwrap(),
        Some(ProjectName {
            slug: "jei".into(),
            title: "Just Enough Items".into()
        })
    );
    assert_eq!(platform.project_name("not-a-number").unwrap(), None);
}

#[test]
fn resolves_an_id_and_a_url_naming_a_file() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/238222")
        .with_body(json!({ "data": { "id": 238222, "slug": "jei" } }).to_string())
        .create();
    server
        .mock("GET", "/v1/mods/238222/files")
        .match_query(Matcher::Any)
        .with_body(json!({ "data": [file(1, 238222, "2024-01-01T00:00:00Z")] }).to_string())
        .create();
    server
        .mock("GET", "/v1/mods/search")
        .match_query(Matcher::Any)
        .with_body(json!({ "data": [{ "id": 238222, "slug": "jei" }] }).to_string())
        .create();
    server
        .mock("GET", "/v1/mods/238222/files/9")
        .with_body(json!({ "data": file(9, 238222, "2024-01-01T00:00:00Z") }).to_string())
        .create();
    let mut wrong = file(10, 238222, "2024-01-01T00:00:00Z");
    wrong["gameVersions"] = json!(["1.20.1", "Forge"]);
    server
        .mock("GET", "/v1/mods/238222/files/10")
        .with_body(json!({ "data": wrong }).to_string())
        .create();

    let cf = with_key(&server);
    assert_eq!(cf.resolve("238222", &pack()).unwrap().len(), 1);
    let url = "https://www.curseforge.com/minecraft/mc-mods/jei/files/9";
    let candidates = cf.resolve(url, &pack()).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].section, section(238222, 9));
    let url = "https://www.curseforge.com/minecraft/mc-mods/jei/files/10";
    assert!(
        cf.resolve(url, &pack()).unwrap().is_empty(),
        "a file for another version or loader is not a candidate"
    );
}

#[test]
fn an_unknown_mod_resolves_to_nothing() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/search")
        .match_query(Matcher::Any)
        .with_body(json!({ "data": [{ "id": 1, "slug": "nope-extra" }] }).to_string())
        .create();
    server.mock("GET", "/v1/mods/123").with_status(404).create();
    let cf = with_key(&server);
    assert!(cf.resolve("nope", &pack()).unwrap().is_empty());
    assert!(cf.resolve("123", &pack()).unwrap().is_empty());
}

#[test]
fn an_unknown_loader_is_refused() {
    let mut pack = pack();
    pack.loader = "rift".into();
    let cf = CurseForge::with("http://unused.invalid", || Some(KEY.into()));
    let err = cf.resolve("jei", &pack).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("rift"));
}

#[test]
fn fills_required_dependencies_only() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/238222/files/5")
        .with_body(json!({ "data": file(5, 238222, "2024-01-01T00:00:00Z") }).to_string())
        .create();
    let mut e = entry(238222, 5);
    with_key(&server).fill_entry(&mut e).unwrap();
    let table = e.platform(NAME).unwrap();
    assert_eq!(table["projectID"].as_integer(), Some(238222));
    assert_eq!(table["fileID"].as_integer(), Some(5));
    let deps: Vec<i64> = table["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_integer().unwrap())
        .collect();
    assert_eq!(deps, [6, 7]);
    assert!(e.downloads.is_empty(), "CurseForge URLs are never stored");
}

#[test]
fn downloads_through_the_api_when_allowed() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/238222/files/5")
        .with_body(json!({ "data": file(5, 238222, "2024-01-01T00:00:00Z") }).to_string())
        .create();
    let Download::Url(url) = with_key(&server).download(&entry(238222, 5)).unwrap() else {
        panic!("CurseForge gives a URL");
    };
    assert_eq!(url, "https://edge.forgecdn.net/files/5/mod-5.jar");
}

#[test]
fn a_file_the_author_keeps_on_curseforge_is_explained() {
    let mut server = Server::new();
    let mut f = file(5, 238222, "2024-01-01T00:00:00Z");
    f["downloadUrl"] = serde_json::Value::Null;
    server
        .mock("GET", "/v1/mods/238222/files/5")
        .with_body(json!({ "data": f }).to_string())
        .create();
    server
        .mock("GET", "/v1/mods/238222")
        .with_body(
            json!({ "data": { "id": 238222, "slug": "jei", "allowModDistribution": false,
                "links": { "websiteUrl": "https://www.curseforge.com/minecraft/mc-mods/jei" } } })
            .to_string(),
        )
        .create();
    let err = with_key(&server).download(&entry(238222, 5)).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("cannot be downloaded again automatically"),
        "{text}"
    );
    assert!(err.hint().contains("mod-5.jar"));
    assert!(
        err.hint()
            .contains("https://www.curseforge.com/minecraft/mc-mods/jei")
    );
    assert!(err.hint().contains("direct URL"));
}

#[test]
fn a_refused_key_says_so_without_showing_it() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/238222/files/5")
        .with_status(403)
        .create();
    let err = with_key(&server).download(&entry(238222, 5)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Usage);
    assert!(err.to_string().contains("refused the API key"));
    assert!(!err.to_string().contains(KEY));
}

#[test]
fn a_vanished_file_is_reported() {
    let mut server = Server::new();
    server
        .mock("GET", "/v1/mods/238222/files/5")
        .with_status(404)
        .create();
    let err = with_key(&server)
        .fill_entry(&mut entry(238222, 5))
        .unwrap_err();
    assert!(err.to_string().contains("no longer exists"));
}

#[test]
fn a_mod_not_on_curseforge_blocks_the_pack() {
    let cf = CurseForge::with("http://unused.invalid", || None);
    let mut e = entry(1, 2);
    e.platforms.clear();
    let Eligibility::Blocked { reason } = cf.pack_eligibility(&e).unwrap() else {
        panic!("a mod off CurseForge is not eligible");
    };
    assert!(reason.contains("approved list"));
}

#[test]
fn parses_queries() {
    assert_eq!(parse_query("jei"), Some((Query::Slug("jei".into()), None)));
    assert_eq!(parse_query(" 238222 "), Some((Query::Id(238222), None)));
    assert_eq!(
        parse_query("https://www.curseforge.com/minecraft/mc-mods/jei/files/5846877?x=1"),
        Some((Query::Slug("jei".into()), Some(5846877)))
    );
    assert_eq!(
        parse_query("https://www.curseforge.com/minecraft/mc-mods/jei"),
        Some((Query::Slug("jei".into()), None))
    );
    assert_eq!(
        parse_query("https://www.curseforge.com/projects/238222"),
        Some((Query::Id(238222), None))
    );
    assert_eq!(parse_query("https://modrinth.com/mod/jei"), None);
    assert_eq!(
        parse_query("https://notcurseforge.com/minecraft/mc-mods/jei"),
        None
    );
}

/// Real API calls, run by hand (`cargo test curseforge -- --ignored`).
/// They skip themselves without a key.
mod real {
    use super::*;

    fn real_key() -> Option<String> {
        api_key().or_else(|| {
            std::env::var("CURSEFORGE_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty())
        })
    }

    /// Resolves JEI, fills and downloads its newest file, then identifies the
    /// downloaded jar: the fingerprint must match the real file exactly.
    #[test]
    #[ignore = "calls the real CurseForge API"]
    fn round_trip_on_jei() {
        if real_key().is_none() {
            eprintln!("skipped: set {KEY_VAR} to run it");
            return;
        }
        let cf = CurseForge::with(BASE_URL, real_key);
        let candidates = cf.resolve("jei", &pack()).unwrap();
        assert!(!candidates.is_empty());
        let mut e = entry(0, 0);
        e.platforms = [(NAME.to_owned(), candidates[0].section.clone())].into();
        cf.fill_entry(&mut e).unwrap();

        let Download::Url(url) = cf.download(&e).unwrap() else {
            panic!("CurseForge gives a URL");
        };
        let mut response = ureq::get(&url).call().unwrap();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024)
            .read_to_vec()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(&candidates[0].file_name);
        fs::write(&path, bytes).unwrap();

        let found = cf.identify(&[jar(&path)]).unwrap();
        let m = found[0].as_ref().expect("the downloaded jar is known");
        assert!(m.exact);
        assert_eq!(m.section, candidates[0].section);
        assert_eq!(m.slug.as_deref(), Some("jei"));
    }
}
