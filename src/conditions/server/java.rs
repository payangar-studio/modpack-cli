//! The Java that runs the server: its version, and the version Minecraft needs.

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use super::{Step, get_json};
use crate::store::download::Fetch;

/// Mojang's list of Minecraft versions, each pointing to its version file.
pub const MOJANG_VERSIONS: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Major version of the Java at `java`, from `java -version`; `None` when
/// there is no Java there.
pub fn major_version(java: &Path) -> Step<Option<u32>> {
    let output = match Command::new(java).arg("-version").output() {
        Ok(output) => output,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "cannot run Java (`{}`): {e}; set JAVA_HOME or put java on the PATH",
                java.display()
            ));
        }
    };
    // `java -version` writes to stderr.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    parse_version(&text).map(Some).ok_or_else(|| {
        format!(
            "cannot read the version of Java (`{}`) from `java -version`: {}",
            java.display(),
            text.lines().next().unwrap_or_default()
        )
    })
}

/// `openjdk version "21.0.5" 2024-10-15` gives 21, `java version "1.8.0_381"`
/// gives 8.
fn parse_version(text: &str) -> Option<u32> {
    let start = text.find("version \"")? + "version \"".len();
    let version = &text[start..];
    let version = &version[..version.find('"')?];
    let mut parts = version.split(['.', '-', '+', '_']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Major Java version Minecraft `minecraft` needs, after Mojang's version file.
pub fn required(minecraft: &str, fetch: &dyn Fetch) -> Step<u32> {
    #[derive(Deserialize)]
    struct Versions {
        versions: Vec<Listed>,
    }
    #[derive(Deserialize)]
    struct Listed {
        id: String,
        url: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Version {
        java_version: Option<JavaVersion>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct JavaVersion {
        major_version: u32,
    }

    let versions: Versions = get_json(fetch, MOJANG_VERSIONS)?;
    let listed = versions
        .versions
        .into_iter()
        .find(|v| v.id == minecraft)
        .ok_or_else(|| format!("Minecraft `{minecraft}` is not in Mojang's version list"))?;
    let version: Version = get_json(fetch, &listed.url)?;
    // Versions older than the field all ran on Java 8.
    Ok(version.java_version.map_or(8, |j| j.major_version))
}

/// Fails when the Java found is older than what Minecraft needs.
pub fn check(have: u32, required: u32, minecraft: &str, java: &Path) -> Step<()> {
    if have >= required {
        return Ok(());
    }
    Err(format!(
        "Java {have} (`{}`) is too old for Minecraft {minecraft}, which needs Java {required} or later; set JAVA_HOME to a Java {required}",
        java.display()
    ))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::store::download::tests::MapFetch;

    #[test]
    fn versions_are_read_old_and_new_style() {
        assert_eq!(
            parse_version("openjdk version \"21.0.5\" 2024-10-15\nOpenJDK Runtime"),
            Some(21)
        );
        assert_eq!(parse_version("java version \"1.8.0_381\""), Some(8));
        assert_eq!(parse_version("openjdk version \"25\" 2025-09-16"), Some(25));
        assert_eq!(parse_version("openjdk version \"23-ea\""), Some(23));
        assert_eq!(parse_version("command not found"), None);
    }

    #[test]
    fn missing_java_has_no_version() {
        assert_eq!(
            major_version(Path::new("/nonexistent/modpack-test/java")),
            Ok(None)
        );
    }

    pub(in crate::conditions) fn mojang(fetch: MapFetch, minecraft: &str, java: u32) -> MapFetch {
        let url = format!("https://piston-meta.invalid/{minecraft}.json");
        fetch
            .with(
                MOJANG_VERSIONS,
                format!(r#"{{"versions":[{{"id":"{minecraft}","url":"{url}"}}]}}"#).as_bytes(),
            )
            .with(
                &url,
                format!(r#"{{"javaVersion":{{"component":"x","majorVersion":{java}}}}}"#)
                    .as_bytes(),
            )
    }

    #[test]
    fn required_version_comes_from_mojang() {
        let fetch = mojang(MapFetch::default(), "1.21.1", 21);
        assert_eq!(required("1.21.1", &fetch), Ok(21));
        let reason = required("9.9", &fetch).unwrap_err();
        assert!(reason.contains("not in Mojang's version list"), "{reason}");

        let old = MapFetch::default()
            .with(
                MOJANG_VERSIONS,
                br#"{"versions":[{"id":"1.6.4","url":"https://m.invalid/1.6.4.json"}]}"#,
            )
            .with("https://m.invalid/1.6.4.json", b"{}");
        assert_eq!(required("1.6.4", &old), Ok(8));
    }

    #[test]
    fn too_old_java_names_both_versions() {
        assert_eq!(check(21, 21, "1.21.1", Path::new("java")), Ok(()));
        let reason = check(17, 21, "1.21.1", Path::new("java")).unwrap_err();
        assert!(
            reason.starts_with(
                "Java 17 (`java`) is too old for Minecraft 1.21.1, which needs Java 21"
            ),
            "{reason}"
        );
    }
}
