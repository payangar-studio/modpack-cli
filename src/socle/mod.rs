//! The socle in a repository: what `modpack init` lays down and `modpack
//! update` rewrites, after the Spec Kit model.
//!
//! The templates are embedded in the binary, in three families:
//! - `templates/socle/`: files the socle owns, written at the same relative
//!   path, recorded in the registry and rewritten by every update;
//! - `templates/blocks/`: blocks between markers in a file the pack owns
//!   otherwise, recorded in the registry as blocks;
//! - `templates/project/`: the skeleton of the pack's content, written once by
//!   `init` when missing, never recorded, never touched again.
//!
//! Adding a template file to `templates/socle/` or `templates/project/` needs
//! no code change.

pub mod blocks;
pub mod migrate;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use include_dir::{Dir, include_dir};

use crate::error::{Context, Error, Result};
use crate::model::manifest::{self, Manifest, Pack};
use crate::model::socle::{
    InitValues, LockedFile, LockedKind, SocleConfig, SocleLock, cli_version, compare_versions,
};
use crate::model::{sha256_bytes, sha256_file};
use blocks::Markers;

static SOCLE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/socle");
static BLOCKS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/blocks");
static PROJECT: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/project");

/// Block templates: template file in `templates/blocks/`, target file in the
/// repository, markers in the target's comment syntax.
const BLOCK_TARGETS: &[(&str, &str, Markers)] = &[
    ("agents.md", "AGENTS.md", blocks::HTML),
    ("gitignore", ".gitignore", blocks::HASH),
];

/// The instructions file, and the link that points Claude at it.
const AGENTS_MD: &str = "AGENTS.md";
const CLAUDE_MD: &str = "CLAUDE.md";

/// A file to write: path relative to the repository root with `/`
/// separators, and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub path: String,
    pub contents: &'static [u8],
}

/// A block to write in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub target: &'static str,
    pub markers: Markers,
    pub content: String,
}

/// Every file of the socle, sorted by path.
pub fn socle_files() -> Vec<Template> {
    files_of(&SOCLE)
}

/// Every file of the pack skeleton, sorted by path.
pub fn project_files() -> Vec<Template> {
    files_of(&PROJECT)
}

/// Every block of the socle.
pub fn socle_blocks() -> Vec<Block> {
    BLOCK_TARGETS
        .iter()
        .map(|(template, target, markers)| {
            let file = BLOCKS
                .get_file(template)
                .unwrap_or_else(|| panic!("templates/blocks/{template} is embedded"));
            let text = file
                .contents_utf8()
                .unwrap_or_else(|| panic!("templates/blocks/{template} is UTF-8"));
            Block {
                target,
                markers: *markers,
                content: blocks::with_final_newline(text),
            }
        })
        .collect()
}

fn files_of(dir: &'static Dir<'static>) -> Vec<Template> {
    fn walk(dir: &'static Dir<'static>, out: &mut Vec<Template>) {
        for file in dir.files() {
            let path = file
                .path()
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Template {
                path,
                contents: file.contents(),
            });
        }
        for sub in dir.dirs() {
            walk(sub, out);
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// A path of the registry or of a template, under the repository root.
fn under(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |p, c| p.join(c))
}

/// Checks the repository's socle version against the CLI's, at the start of
/// every command that works in a repository: a newer socle is refused, an
/// older one gives a warning, once per process, that suggests `modpack update`.
/// A repository `modpack update` has not migrated yet is refused: without
/// `variants.toml`, the commands would skip its checks or start a new one.
pub fn check_version(root: &Path) -> Result<()> {
    static WARNED: Once = Once::new();
    if root.join(migrate::OLD_FILE).is_file() {
        return Err(Error::usage(
            format!(
                "this repository still holds {}, written before variants",
                migrate::OLD_FILE
            ),
            "run `modpack update` to rename it to variants.toml",
        ));
    }
    let config = SocleConfig::load(root)?;
    if let Some(warning) = compare_versions(&config.version, &cli_version())?.enforce()? {
        WARNED.call_once(|| tracing::warn!("{warning}"));
    }
    Ok(())
}

/// Creates a modpack repository in `root`, which must not hold `.modpack/`
/// yet: socle settings, empty manifest, pack skeleton, socle files and
/// blocks, registry, and `CLAUDE.md` pointing at `AGENTS.md`.
///
/// Pack files that already exist (the manifest, the skeleton) are kept as
/// they are: the socle never overwrites the pack's content.
pub fn init(root: &Path, values: InitValues) -> Result<()> {
    let manifest_path = root.join(manifest::FILE_NAME);
    if !manifest_path.exists() {
        Manifest::new(Pack {
            minecraft: values.minecraft.clone(),
            loader: values.loader.clone(),
            loader_version: values.loader_version.clone(),
        })
        .save(&manifest_path)?;
    }
    for template in project_files() {
        let path = under(root, &template.path);
        if !path.exists() {
            write(&path, template.contents)?;
        }
    }
    write_socle(root)?.save(root)?;
    link_claude_md(root)?;
    SocleConfig {
        version: cli_version().to_string(),
        init: values,
    }
    .save(root)
}

/// Writes every socle file and block, and returns the registry of what was
/// written.
pub fn write_socle(root: &Path) -> Result<SocleLock> {
    let mut lock = SocleLock::default();
    for template in socle_files() {
        write(&under(root, &template.path), template.contents)?;
        lock.files.push(LockedFile {
            sha256: sha256_bytes(template.contents),
            path: template.path,
            kind: LockedKind::File,
        });
    }
    for block in socle_blocks() {
        let path = under(root, block.target);
        let text = read_if_exists(&path)?.unwrap_or_default();
        write(
            &path,
            blocks::upsert(&text, block.markers, &block.content).as_bytes(),
        )?;
        lock.files.push(LockedFile {
            path: block.target.to_owned(),
            sha256: sha256_bytes(block.content.as_bytes()),
            kind: LockedKind::Block,
        });
    }
    Ok(lock)
}

/// Registry entries whose file or block no longer matches the recorded hash:
/// edited by hand, deleted, or with a marker gone.
pub fn edited_by_hand(root: &Path, lock: &SocleLock) -> Result<Vec<String>> {
    let mut edited = Vec::new();
    for entry in &lock.files {
        let path = under(root, &entry.path);
        let current = match entry.kind {
            LockedKind::File if path.is_file() => {
                Some(sha256_file(&path).context(format!("reading {}", path.display()))?)
            }
            LockedKind::File => None,
            LockedKind::Block => read_if_exists(&path)?.and_then(|text| {
                blocks::find(&text, markers_of(&entry.path)).map(|c| sha256_bytes(c.as_bytes()))
            }),
        };
        if current.as_deref() != Some(entry.sha256.as_str()) {
            edited.push(entry.path.clone());
        }
    }
    Ok(edited)
}

/// Removes what `old` recorded and `new` no longer has: socle files that left
/// the templates, and blocks whose template is gone. Returns their paths.
pub fn remove_stale(root: &Path, old: &SocleLock, new: &SocleLock) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for entry in &old.files {
        let still = new
            .files
            .iter()
            .any(|n| n.path == entry.path && n.kind == entry.kind);
        if still {
            continue;
        }
        let path = under(root, &entry.path);
        match entry.kind {
            LockedKind::File => {
                if path.is_file() {
                    fs::remove_file(&path).context(format!("removing {}", path.display()))?;
                }
                remove_empty_parents(root, &path);
            }
            LockedKind::Block => {
                if let Some(text) = read_if_exists(&path)? {
                    write(
                        &path,
                        blocks::remove(&text, markers_of(&entry.path)).as_bytes(),
                    )?;
                }
            }
        }
        removed.push(entry.path.clone());
    }
    Ok(removed)
}

/// Markers of a block's target file: the known targets first, then by the
/// file's extension.
fn markers_of(target: &str) -> Markers {
    BLOCK_TARGETS
        .iter()
        .find(|(_, t, _)| *t == target)
        .map(|(_, _, m)| *m)
        .unwrap_or(if target.ends_with(".md") {
            blocks::HTML
        } else {
            blocks::HASH
        })
}

/// Removes the directories left empty between `file` and `root`.
fn remove_empty_parents(root: &Path, file: &Path) {
    for dir in file.ancestors().skip(1) {
        if dir == root || !dir.starts_with(root) || fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

fn read_if_exists(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context(format!("reading {}", path.display())),
    }
}

/// Writes a file, creating its directory. A template starting with `#!` is a
/// script (a git hook): it is made executable where that means something.
fn write(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(format!("creating {}", dir.display()))?;
    }
    fs::write(path, contents).context(format!("writing {}", path.display()))?;
    #[cfg(unix)]
    if contents.starts_with(b"#!") {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .context(format!("making {} executable", path.display()))?;
    }
    Ok(())
}

/// Points `CLAUDE.md` at `AGENTS.md`, unless something named `CLAUDE.md`
/// already exists.
fn link_claude_md(root: &Path) -> Result<()> {
    let link = root.join(CLAUDE_MD);
    if link.symlink_metadata().is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(AGENTS_MD, &link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(AGENTS_MD, &link);
    #[cfg(not(any(unix, windows)))]
    let made: std::io::Result<()> = Err(std::io::ErrorKind::Unsupported.into());
    if let Err(e) = made {
        tracing::warn!("could not link {CLAUDE_MD} to {AGENTS_MD}: {e}; create the link yourself");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::variants::Variants;

    fn values() -> InitValues {
        InitValues {
            name: "Test Pack".into(),
            minecraft: "1.21.1".into(),
            loader: "neoforge".into(),
            loader_version: "21.1.77".into(),
            author: "Tester".into(),
        }
    }

    fn initialised() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path(), values()).unwrap();
        dir
    }

    #[test]
    fn hidden_template_files_are_embedded() {
        let paths: Vec<_> = socle_files().into_iter().map(|t| t.path).collect();
        assert!(paths.contains(&".modpack/README.md".to_owned()));
        let paths: Vec<_> = project_files().into_iter().map(|t| t.path).collect();
        for path in [
            "config/.gitkeep",
            "mods/.gitkeep",
            "variants.toml",
            "overrides/.gitkeep",
        ] {
            assert!(paths.contains(&path.to_owned()), "{path} missing");
        }
    }

    #[test]
    fn every_block_template_has_a_target() {
        for file in BLOCKS.files() {
            let name = file.path().to_str().unwrap();
            assert!(
                BLOCK_TARGETS.iter().any(|(t, _, _)| *t == name),
                "templates/blocks/{name} has no entry in BLOCK_TARGETS"
            );
        }
        assert_eq!(socle_blocks().len(), BLOCK_TARGETS.len());
    }

    #[test]
    fn skeleton_variants_have_a_default_variant_and_no_condition() {
        let template = project_files()
            .into_iter()
            .find(|t| t.path == "variants.toml")
            .unwrap();
        let variants = Variants::parse(std::str::from_utf8(template.contents).unwrap()).unwrap();
        assert!(variants.conditions.is_empty());
        assert!(variants.default.conditions.is_empty());
        assert!(variants.default.remove.is_empty());
        assert!(variants.variants.is_empty());
    }

    #[test]
    fn init_records_every_file_it_lays_down() {
        let dir = initialised();
        let root = dir.path();
        let lock = SocleLock::load(root).unwrap();
        assert_eq!(lock.files.len(), socle_files().len() + socle_blocks().len());
        assert!(edited_by_hand(root, &lock).unwrap().is_empty());
        for entry in &lock.files {
            if entry.kind == LockedKind::File {
                assert_eq!(
                    sha256_file(&under(root, &entry.path)).unwrap(),
                    entry.sha256
                );
            }
        }
        let config = SocleConfig::load(root).unwrap();
        assert_eq!(config.version, cli_version().to_string());
        assert_eq!(config.init, values());
        let manifest = Manifest::load(&root.join(manifest::FILE_NAME)).unwrap();
        assert_eq!(manifest.pack.loader_version, "21.1.77");
        assert!(manifest.mods.is_empty());
        assert!(root.join("variants.toml").is_file());
        assert!(root.join("overrides").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn init_links_claude_md_to_agents_md() {
        let dir = initialised();
        let link = dir.path().join(CLAUDE_MD);
        assert_eq!(fs::read_link(&link).unwrap(), Path::new(AGENTS_MD));
    }

    #[test]
    fn init_keeps_existing_pack_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("variants.toml"), "# mine\n").unwrap();
        fs::write(dir.path().join(CLAUDE_MD), "mine\n").unwrap();
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        init(dir.path(), values()).unwrap();
        let read = |p: &str| fs::read_to_string(dir.path().join(p)).unwrap();
        assert_eq!(read("variants.toml"), "# mine\n");
        assert_eq!(read(CLAUDE_MD), "mine\n");
        assert!(read(".gitignore").starts_with("target/\n\n# modpack:begin\n"));
    }

    #[test]
    fn hand_edits_are_detected() {
        let dir = initialised();
        let root = dir.path();
        let lock = SocleLock::load(root).unwrap();

        // Text around a block belongs to the pack.
        let agents = root.join(AGENTS_MD);
        let text = fs::read_to_string(&agents).unwrap();
        fs::write(&agents, format!("# Mine\n\n{text}\nMore.\n")).unwrap();
        assert!(edited_by_hand(root, &lock).unwrap().is_empty());

        fs::write(root.join(".modpack/README.md"), "edited\n").unwrap();
        fs::write(
            &agents,
            blocks::upsert(&text, blocks::HTML, "edited block\n"),
        )
        .unwrap();
        fs::remove_file(root.join(".gitignore")).unwrap();
        assert_eq!(
            edited_by_hand(root, &lock).unwrap(),
            [".gitignore", ".modpack/README.md", AGENTS_MD]
        );
    }

    #[test]
    fn stale_files_and_blocks_are_removed() {
        let dir = initialised();
        let root = dir.path();
        let new = SocleLock::load(root).unwrap();
        let mut old = new.clone();
        write(&root.join(".modpack/gone/old.sh"), b"#!/bin/sh\n").unwrap();
        old.files.push(LockedFile {
            path: ".modpack/gone/old.sh".into(),
            sha256: sha256_bytes(b"#!/bin/sh\n"),
            kind: LockedKind::File,
        });
        fs::write(
            root.join("NOTES.md"),
            "Mine.\n<!-- modpack:begin -->\nold\n<!-- modpack:end -->\n",
        )
        .unwrap();
        old.files.push(LockedFile {
            path: "NOTES.md".into(),
            sha256: sha256_bytes(b"old\n"),
            kind: LockedKind::Block,
        });

        let removed = remove_stale(root, &old, &new).unwrap();
        assert_eq!(removed, [".modpack/gone/old.sh", "NOTES.md"]);
        assert!(!root.join(".modpack/gone").exists());
        assert!(root.join(".modpack").is_dir());
        assert_eq!(
            fs::read_to_string(root.join("NOTES.md")).unwrap(),
            "Mine.\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn scripts_are_written_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook");
        write(&path, b"#!/bin/sh\n").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
    }

    #[test]
    fn version_check_refuses_a_newer_repository() {
        let dir = initialised();
        let mut config = SocleConfig::load(dir.path()).unwrap();
        check_version(dir.path()).unwrap();
        config.version = "999.0.0".into();
        config.save(dir.path()).unwrap();
        let err = check_version(dir.path()).unwrap_err();
        assert!(err.to_string().contains("newer than this CLI"));
        config.version = "0.0.1".into();
        config.save(dir.path()).unwrap();
        check_version(dir.path()).unwrap();
    }

    #[test]
    fn version_check_refuses_a_repository_not_migrated_to_variants() {
        let dir = initialised();
        fs::write(dir.path().join("modes.toml"), "[default]\n").unwrap();
        let err = check_version(dir.path()).unwrap_err();
        assert!(err.to_string().contains("modes.toml"), "{err}");
        assert!(err.hint().contains("modpack update"), "{err}");
    }
}
