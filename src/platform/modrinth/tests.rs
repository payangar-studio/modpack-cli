//! Operations against a mock of the Modrinth API, and one test against the
//! real API (`#[ignore]`, run by hand).

use std::io::Read;

use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use tokio::runtime::Runtime;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::model::manifest::Hashes;
use crate::platform::Registry;

const JAR: &[u8] = b"sodium jar bytes";

fn sha512(bytes: &[u8]) -> String {
    Sha512::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

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

    fn platform(&self) -> Modrinth {
        Modrinth::new(&self.server.uri())
    }

    fn requests(&self) -> usize {
        self.rt
            .block_on(self.server.received_requests())
            .unwrap()
            .len()
    }

    fn file_url(&self) -> String {
        format!("{}/data/AANobbMI/versions/v2/sodium.jar", self.server.uri())
    }
}

fn pack() -> Pack {
    Pack {
        minecraft: "1.21.1".into(),
        loader: "neoforge".into(),
        loader_version: "21.1.77".into(),
    }
}

fn project_json() -> Value {
    json!({
        "id": "AANobbMI",
        "slug": "sodium",
        "title": "Sodium",
        "client_side": "required",
        "server_side": "unsupported",
    })
}

fn version_json(api: &Api, id: &str, number: &str, version_type: &str) -> Value {
    json!({
        "id": id,
        "project_id": "AANobbMI",
        "version_number": number,
        "version_type": version_type,
        "files": [
            {
                "hashes": { "sha1": "b".repeat(40), "sha512": "c".repeat(128) },
                "url": format!("{}/sources.jar", api.server.uri()),
                "filename": "sodium-sources.jar",
                "primary": false,
                "size": 3,
            },
            {
                "hashes": { "sha1": "a".repeat(40), "sha512": sha512(JAR) },
                "url": api.file_url(),
                "filename": format!("sodium-{number}.jar"),
                "primary": true,
                "size": JAR.len(),
            },
        ],
        "dependencies": [
            { "project_id": "P7dR8mSH", "version_id": null, "dependency_type": "required" },
            { "project_id": null, "version_id": "depver01", "dependency_type": "required" },
            { "project_id": "optional1", "version_id": null, "dependency_type": "optional" },
            { "project_id": "AANobbMI", "version_id": null, "dependency_type": "required" },
        ],
    })
}

fn entry(section: toml::Table) -> Entry {
    Entry {
        key: "sodium".into(),
        path: "mods/sodium-0.6.0.jar".into(),
        file_size: 0,
        downloads: Vec::new(),
        url: None,
        hashes: Hashes {
            sha1: String::new(),
            sha512: String::new(),
        },
        env: Env::default(),
        platforms: [(NAME.to_owned(), section)].into(),
    }
}

fn jar(sha512: String) -> Jar {
    Jar {
        path: "mods/x.jar".into(),
        sha1: String::new(),
        sha512,
        size: 1,
    }
}

#[test]
fn registered_under_its_name_with_page_by_api() {
    let registry = Registry::builtin();
    let modrinth = registry.get("modrinth").unwrap();
    for (_, capability) in Registry::capabilities(modrinth) {
        assert_eq!(capability, Capability::Api);
    }
}

#[test]
fn identify_finds_known_hashes_and_skips_unknown_ones() {
    let api = Api::start();
    let known = sha512(JAR);
    let unknown = "0".repeat(128);
    api.mount(
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .and(header("user-agent", client::USER_AGENT))
            .and(body_json(
                json!({ "hashes": [known, unknown], "algorithm": "sha512" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                known.clone(): version_json(&api, "Pb3OXVqC", "0.6.0", "release"),
            }))),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/projects"))
            .and(query_param("ids", r#"["AANobbMI"]"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([project_json()]))),
    );

    let found = api
        .platform()
        .identify(&[jar(known), jar(unknown)])
        .unwrap();
    let hit = found[0].as_ref().unwrap();
    assert_eq!(hit.slug.as_deref(), Some("sodium"));
    assert!(hit.exact);
    assert_eq!(hit.section, section("AANobbMI", "Pb3OXVqC"));
    assert!(found[1].is_none());
}

#[test]
fn identify_with_no_known_hash_skips_the_project_lookup() {
    let api = Api::start();
    api.mount(
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({}))),
    );
    let found = api.platform().identify(&[jar("0".repeat(128))]).unwrap();
    assert_eq!(found, [None]);
    assert_eq!(api.requests(), 1);
}

#[test]
fn resolve_lists_compatible_versions_releases_first() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/sodium"))
            .respond_with(ResponseTemplate::new(200).set_body_json(project_json())),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/AANobbMI/version"))
            .and(query_param("game_versions", r#"["1.21.1"]"#))
            .and(query_param("loaders", r#"["neoforge"]"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                version_json(&api, "beta0001", "0.6.1-beta", "beta"),
                version_json(&api, "Pb3OXVqC", "0.6.0", "release"),
            ]))),
    );

    let platform = api.platform();
    for query in [
        "sodium",
        "https://modrinth.com/mod/sodium",
        "modrinth.com/mod/sodium/version/0.6.0",
    ] {
        let candidates = platform.resolve(query, &pack()).unwrap();
        assert_eq!(candidates.len(), 2, "{query}");
        assert_eq!(candidates[0].label, "0.6.0");
        assert_eq!(candidates[0].slug.as_deref(), Some("sodium"));
        assert_eq!(candidates[0].file_name, "sodium-0.6.0.jar");
        assert_eq!(candidates[0].section, section("AANobbMI", "Pb3OXVqC"));
        assert_eq!(candidates[1].label, "0.6.1-beta");
    }
}

#[test]
fn resolve_of_an_unknown_mod_or_a_foreign_url_is_empty() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/nope"))
            .respond_with(ResponseTemplate::new(404)),
    );
    let platform = api.platform();
    assert!(platform.resolve("nope", &pack()).unwrap().is_empty());
    assert!(
        platform
            .resolve("https://www.curseforge.com/minecraft/mc-mods/jei", &pack())
            .unwrap()
            .is_empty()
    );
    assert_eq!(api.requests(), 1, "a foreign URL never reaches Modrinth");
}

#[test]
fn fill_entry_sets_downloads_sides_hashes_and_required_dependencies() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/version/Pb3OXVqC"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(version_json(&api, "Pb3OXVqC", "0.6.0", "release")),
            ),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/AANobbMI"))
            .respond_with(ResponseTemplate::new(200).set_body_json(project_json())),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/versions"))
            .and(query_param("ids", r#"["depver01"]"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": "depver01",
                "project_id": "Aa1Bb2Cc",
                "version_number": "1.0",
                "files": [],
            }]))),
    );

    let mut entry = entry(section("AANobbMI", "Pb3OXVqC"));
    api.platform().fill_entry(&mut entry).unwrap();
    assert_eq!(entry.downloads, [api.file_url()]);
    assert_eq!(entry.hashes.sha512, sha512(JAR));
    assert_eq!(entry.hashes.sha1, "a".repeat(40));
    assert_eq!(entry.file_size, JAR.len() as u64);
    assert_eq!(entry.env.client, Support::Required);
    assert_eq!(entry.env.server, Support::Unsupported);
    let table = entry.platform(NAME).unwrap();
    assert_eq!(table["project_id"].as_str(), Some("AANobbMI"));
    assert_eq!(table["version_id"].as_str(), Some("Pb3OXVqC"));
    let deps: Vec<&str> = table["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        deps,
        ["Aa1Bb2Cc", "P7dR8mSH"],
        "required only, self excluded"
    );
}

#[test]
fn fill_entry_refuses_a_jar_that_is_not_in_the_version() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/version/Pb3OXVqC"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(version_json(&api, "Pb3OXVqC", "0.6.0", "release")),
            ),
    );
    let mut entry = entry(section("AANobbMI", "Pb3OXVqC"));
    entry.hashes.sha512 = "d".repeat(128);
    let err = api.platform().fill_entry(&mut entry).unwrap_err();
    assert!(
        err.to_string()
            .contains("is not a file of its Modrinth version")
    );
}

#[test]
fn fill_entry_without_a_modrinth_table_says_so() {
    let mut entry = entry(toml::Table::new());
    entry.platforms.clear();
    let err = Modrinth::new("http://127.0.0.1:9")
        .fill_entry(&mut entry)
        .unwrap_err();
    assert!(err.to_string().contains("is not on Modrinth"));
}

#[test]
fn download_streams_and_checks_the_sha512() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/data/AANobbMI/versions/v2/sodium.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(JAR)),
    );
    let mut entry = entry(section("AANobbMI", "Pb3OXVqC"));
    entry.downloads = vec![api.file_url()];
    entry.hashes.sha512 = sha512(JAR);

    let platform = api.platform();
    let Download::Stream(mut stream) = platform.download(&entry).unwrap() else {
        panic!("Modrinth streams its downloads");
    };
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, JAR);

    entry.hashes.sha512 = sha512(b"another jar");
    let Download::Stream(mut stream) = platform.download(&entry).unwrap() else {
        panic!("Modrinth streams its downloads");
    };
    let err = stream.read_to_end(&mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("sha512 mismatch"));
}

#[test]
fn download_without_url_asks_the_version() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/version/Pb3OXVqC"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(version_json(&api, "Pb3OXVqC", "0.6.0", "release")),
            ),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/data/AANobbMI/versions/v2/sodium.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(JAR)),
    );
    let mut entry = entry(section("AANobbMI", "Pb3OXVqC"));
    entry.hashes.sha512 = sha512(JAR);
    let Download::Stream(mut stream) = api.platform().download(&entry).unwrap() else {
        panic!("Modrinth streams its downloads");
    };
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, JAR);
}

#[test]
fn rate_limit_is_waited_out_then_retried() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/sodium"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("X-Ratelimit-Limit", "300")
                    .insert_header("X-Ratelimit-Remaining", "0")
                    .insert_header("X-Ratelimit-Reset", "0"),
            )
            .up_to_n_times(1),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/sodium"))
            .respond_with(ResponseTemplate::new(200).set_body_json(project_json())),
    );
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/AANobbMI/version"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([]))),
    );
    assert!(
        api.platform()
            .resolve("sodium", &pack())
            .unwrap()
            .is_empty()
    );
    assert_eq!(api.requests(), 3);
}

#[test]
fn rate_limit_that_persists_gives_up_with_a_clear_error() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/sodium"))
            .respond_with(ResponseTemplate::new(429).insert_header("X-Ratelimit-Reset", "0")),
    );
    let err = api.platform().resolve("sodium", &pack()).unwrap_err();
    assert!(err.to_string().contains("rate limit"));
    assert_eq!(api.requests(), 4, "first call plus three retries");
}

#[test]
fn server_error_is_reported() {
    let api = Api::start();
    api.mount(
        Mock::given(method("GET"))
            .and(path("/project/sodium"))
            .respond_with(ResponseTemplate::new(500)),
    );
    let err = api.platform().resolve("sodium", &pack()).unwrap_err();
    assert!(err.to_string().contains("500"));
}

#[test]
fn eligibility_follows_the_download_hosts() {
    let platform = Modrinth::new("http://127.0.0.1:9");
    let mut entry = entry(section("AANobbMI", "Pb3OXVqC"));
    entry.downloads = vec!["https://cdn.modrinth.com/data/AANobbMI/x.jar".into()];
    assert_eq!(
        platform.pack_eligibility(&entry).unwrap(),
        Eligibility::Eligible
    );
    entry.downloads = vec!["https://example.org/x.jar".into()];
    let Eligibility::Blocked { reason } = platform.pack_eligibility(&entry).unwrap() else {
        panic!("a mod hosted elsewhere must be embedded");
    };
    assert!(reason.contains("embedded"));
}

#[test]
fn push_page_is_declared_but_not_implemented_yet() {
    let err = Modrinth::default().push_page(Path::new(".")).unwrap_err();
    assert!(err.to_string().contains("not implemented yet"));
}

#[test]
fn parse_query_reads_slugs_ids_and_modrinth_urls() {
    assert_eq!(parse_query("sodium").as_deref(), Some("sodium"));
    assert_eq!(parse_query(" AANobbMI ").as_deref(), Some("AANobbMI"));
    assert_eq!(
        parse_query("https://modrinth.com/project/AANobbMI?tab=versions").as_deref(),
        Some("AANobbMI")
    );
    assert_eq!(
        parse_query("https://www.modrinth.com/datapack/terralith").as_deref(),
        Some("terralith")
    );
    assert_eq!(parse_query("https://modrinth.com/mod"), None);
    assert_eq!(parse_query("https://github.com/x/y"), None);
    assert_eq!(parse_query(""), None);
}

#[test]
fn encode_segment_escapes_reserved_characters() {
    assert_eq!(encode_segment("a-b_c.d~e"), "a-b_c.d~e");
    assert_eq!(encode_segment("a/b?c"), "a%2Fb%3Fc");
}

/// Against the real API: resolve, fill, identify and download a known mod.
#[test]
#[ignore = "calls the real Modrinth API; run by hand"]
fn real_api_round_trip_on_sodium() {
    let platform = Modrinth::default();
    let pack = Pack {
        minecraft: "1.21.1".into(),
        loader: "neoforge".into(),
        loader_version: "21.1.77".into(),
    };
    let candidates = platform.resolve("sodium", &pack).unwrap();
    let first = candidates
        .first()
        .expect("sodium exists for 1.21.1 NeoForge");
    assert_eq!(first.slug.as_deref(), Some("sodium"));

    let mut entry = entry(first.section.clone());
    platform.fill_entry(&mut entry).unwrap();
    assert_eq!(
        entry.platform(NAME).unwrap()["project_id"].as_str(),
        Some("AANobbMI")
    );
    assert_eq!(entry.env.server, Support::Unsupported);
    assert_eq!(
        platform.pack_eligibility(&entry).unwrap(),
        Eligibility::Eligible
    );

    let found = platform
        .identify(&[jar(entry.hashes.sha512.clone())])
        .unwrap();
    assert_eq!(found[0].as_ref().unwrap().slug.as_deref(), Some("sodium"));

    let Download::Stream(mut stream) = platform.download(&entry).unwrap() else {
        panic!("Modrinth streams its downloads");
    };
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() as u64, entry.file_size);
}
