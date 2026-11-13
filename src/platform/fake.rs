//! An in-memory platform for tests: it knows the mods it is given, never
//! touches the network, and writes a plain-text "pack".
//!
//! Its manifest table holds `id` (the slug) and, once filled, `dependencies`
//! (keys of required mods).

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use sha2::{Digest, Sha512};

use super::{
    Candidate, Capability, Download, Eligibility, Jar, Match, Operation, PackBuild, Platform,
    ProjectName,
};
use crate::error::{Error, Result};
use crate::model::manifest::{Entry, Env, Pack};

/// A mod the fake platform hosts.
#[derive(Debug, Clone)]
pub struct FakeMod {
    pub slug: String,
    pub title: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub env: Env,
    /// Slugs of its required dependencies.
    pub dependencies: Vec<String>,
}

impl FakeMod {
    /// A mod whose jar is `<slug>-1.0.0.jar` holding its slug as bytes,
    /// required on both sides, without dependencies.
    pub fn new(slug: &str) -> Self {
        Self {
            slug: slug.to_owned(),
            title: format!("Title of {slug}"),
            file_name: format!("{slug}-1.0.0.jar"),
            bytes: slug.as_bytes().to_vec(),
            env: Env::default(),
            dependencies: Vec::new(),
        }
    }

    pub fn sha512(&self) -> String {
        Sha512::digest(&self.bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn section(&self) -> toml::Table {
        let mut section = toml::Table::new();
        section.insert("id".into(), self.slug.clone().into());
        section
    }
}

#[derive(Debug, Clone)]
pub struct FakePlatform {
    pub name: String,
    pub mods: Vec<FakeMod>,
}

impl FakePlatform {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            mods: Vec::new(),
        }
    }

    pub fn with(mut self, m: FakeMod) -> Self {
        self.mods.push(m);
        self
    }

    fn by_slug(&self, slug: &str) -> Option<&FakeMod> {
        self.mods.iter().find(|m| m.slug == slug)
    }

    /// The hosted mod an entry's table points to.
    fn of_entry(&self, entry: &Entry) -> Result<&FakeMod> {
        entry
            .platform(&self.name)
            .and_then(|t| t.get("id"))
            .and_then(toml::Value::as_str)
            .and_then(|id| self.by_slug(id))
            .ok_or_else(|| {
                Error::usage(
                    format!("`{}` is not on platform `{}`", entry.key, self.name),
                    "add it from another platform",
                )
            })
    }
}

impl Platform for FakePlatform {
    fn name(&self) -> &str {
        &self.name
    }

    fn capability(&self, op: Operation) -> Capability {
        match op {
            Operation::PushPage => Capability::Skill("modpack-fake-page"),
            _ => Capability::Api,
        }
    }

    fn identify(&self, jars: &[Jar]) -> Result<Vec<Option<Match>>> {
        Ok(jars
            .iter()
            .map(|jar| {
                self.mods
                    .iter()
                    .find(|m| m.sha512() == jar.sha512)
                    .map(|m| Match {
                        slug: Some(m.slug.clone()),
                        section: m.section(),
                        exact: true,
                    })
            })
            .collect())
    }

    fn resolve(&self, query: &str, _pack: &Pack) -> Result<Vec<Candidate>> {
        Ok(self
            .by_slug(query)
            .map(|m| Candidate {
                label: m.file_name.clone(),
                slug: Some(m.slug.clone()),
                file_name: m.file_name.clone(),
                section: m.section(),
            })
            .into_iter()
            .collect())
    }

    fn fill_entry(&self, entry: &mut Entry) -> Result<()> {
        let m = self.of_entry(entry)?.clone();
        entry.downloads = vec![format!(
            "https://{}.invalid/{}/{}",
            self.name, m.slug, m.file_name
        )];
        entry.env = m.env;
        let deps = m
            .dependencies
            .iter()
            .cloned()
            .map(toml::Value::from)
            .collect();
        entry
            .platform_mut(&self.name)
            .insert("dependencies".into(), toml::Value::Array(deps));
        Ok(())
    }

    fn project_name(&self, id: &str) -> Result<Option<ProjectName>> {
        Ok(self.by_slug(id).map(|m| ProjectName {
            slug: m.slug.clone(),
            title: m.title.clone(),
        }))
    }

    fn download(&self, entry: &Entry) -> Result<Download> {
        let bytes = self.of_entry(entry)?.bytes.clone();
        Ok(Download::Stream(Box::new(Cursor::new(bytes))))
    }

    fn pack_eligibility(&self, entry: &Entry) -> Result<Eligibility> {
        Ok(match self.of_entry(entry) {
            Ok(_) => Eligibility::Eligible,
            Err(_) => Eligibility::Blocked {
                reason: format!("not hosted on {}", self.name),
            },
        })
    }

    /// Writes `<stem>.<name>`: one line per pack field, mod key and override.
    fn build_pack(&self, build: &PackBuild<'_>) -> Result<PathBuf> {
        let mut lines = vec![
            format!("name {}", build.name),
            format!("version {}", build.version),
            format!("minecraft {}", build.pack.minecraft),
        ];
        lines.extend(build.mods.iter().map(|e| format!("mod {}", e.key)));
        lines.extend(
            build
                .overrides
                .iter()
                .map(|o| format!("override {}", o.path)),
        );
        let path = build
            .out_dir
            .join(format!("{}.{}", build.file_stem, self.name));
        fs::write(&path, lines.join("\n") + "\n")?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::manifest::{Hashes, Manifest, Support};
    use std::io::Read;
    use std::path::Path;

    fn platform() -> FakePlatform {
        let mut sodium = FakeMod::new("sodium");
        sodium.env.server = Support::Unsupported;
        sodium.dependencies = vec!["fabric-api".into()];
        FakePlatform::new("fake")
            .with(sodium)
            .with(FakeMod::new("fabric-api"))
    }

    fn pack() -> Pack {
        Pack {
            minecraft: "1.21.1".into(),
            loader: "fabric".into(),
            loader_version: "0.16.0".into(),
        }
    }

    fn entry_from(candidate: &Candidate) -> Entry {
        Entry {
            key: candidate.slug.clone().unwrap(),
            path: format!("mods/{}", candidate.file_name),
            file_size: 0,
            downloads: Vec::new(),
            url: None,
            hashes: Hashes {
                sha1: String::new(),
                sha512: String::new(),
            },
            env: Env::default(),
            platforms: [("fake".to_owned(), candidate.section.clone())].into(),
        }
    }

    #[test]
    fn identifies_known_jars_only() {
        let p = platform();
        let jar = |sha512: String| Jar {
            path: "x.jar".into(),
            sha1: String::new(),
            sha512,
            size: 1,
        };
        let found = p
            .identify(&[jar(FakeMod::new("sodium").sha512()), jar("0".into())])
            .unwrap();
        assert_eq!(found[0].as_ref().unwrap().slug.as_deref(), Some("sodium"));
        assert!(found[1].is_none());
    }

    #[test]
    fn resolves_fills_and_downloads() {
        let p = platform();
        let candidates = p.resolve("sodium", &pack()).unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(p.resolve("nope", &pack()).unwrap().is_empty());

        let mut entry = entry_from(&candidates[0]);
        p.fill_entry(&mut entry).unwrap();
        assert_eq!(entry.env.server, Support::Unsupported);
        assert_eq!(entry.downloads.len(), 1);
        let deps = entry.platform("fake").unwrap()["dependencies"]
            .as_array()
            .unwrap();
        assert_eq!(deps[0].as_str(), Some("fabric-api"));

        let Download::Stream(mut stream) = p.download(&entry).unwrap() else {
            panic!("the fake platform streams");
        };
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"sodium");
    }

    #[test]
    fn entries_written_by_the_fake_survive_the_manifest() {
        let p = platform();
        let mut entry = entry_from(&p.resolve("sodium", &pack()).unwrap()[0]);
        p.fill_entry(&mut entry).unwrap();
        let mut manifest = Manifest::new(pack());
        manifest.upsert(entry);
        assert_eq!(Manifest::parse(&manifest.to_toml()).unwrap(), manifest);
    }

    #[test]
    fn eligibility_and_pack() {
        let p = platform();
        let hosted = entry_from(&p.resolve("sodium", &pack()).unwrap()[0]);
        let mut foreign = hosted.clone();
        foreign.key = "other".into();
        foreign.platforms.clear();
        assert_eq!(p.pack_eligibility(&hosted).unwrap(), Eligibility::Eligible);
        assert!(matches!(
            p.pack_eligibility(&foreign).unwrap(),
            Eligibility::Blocked { .. }
        ));

        let dir = tempfile::tempdir().unwrap();
        let pack = pack();
        let path = p
            .build_pack(&PackBuild {
                name: "Test",
                version: "1.0.0",
                pack: &pack,
                mods: vec![&hosted],
                overrides: Vec::new(),
                out_dir: dir.path(),
                file_stem: "test-default-1.0.0",
            })
            .unwrap();
        assert_eq!(path, dir.path().join("test-default-1.0.0.fake"));
        assert!(fs::read_to_string(path).unwrap().contains("mod sodium"));
        assert!(p.push_page(Path::new(".")).is_err());
    }
}
