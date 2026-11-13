//! Adding a mod to the repository, and removing one.
//!
//! A mod comes from a platform (slug, id or URL, resolved by the first
//! platform of the registry that knows it) or from a local jar. Either way its
//! jar lands in `mods/`, every platform that knows it completes its entry, and
//! the manifest is written. A jar already in `mods-local/` is a local mod,
//! tracked by git; a mod added from a platform under the key of a local mod
//! replaces it, and its jar leaves git. A mod for one mode only is removed
//! from the default mode and added to that mode in `modes.toml`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use super::local::remove_jar;
use super::manifest::{Draft, complete, first_line, load_or_new};
use super::modes_edit;
use crate::error::{Context, Error, Result};
use crate::model::manifest::{self, Entry, Env, Hashes, Manifest, Pack, normalize_key};
use crate::model::modes::DEFAULT_MODE;
use crate::platform::{Candidate, Download, Platform, ProjectName};
use crate::prompt::Prompt;
use crate::store::download::{Sources, download};
use crate::store::hash::{copy_hashing, hash_file};
use crate::store::{LOCAL_DIR, MODS_DIR, jar_path, write_atomic};

/// Fields a platform table names its project by: `project_id` for Modrinth,
/// `projectID` for CurseForge, `id` for the test platform. A table's
/// `dependencies` lists such ids, on the same platform.
const PROJECT_ID_FIELDS: [&str; 3] = ["project_id", "projectID", "id"];

/// What `modpack add` was asked.
#[derive(Debug, Clone, Default)]
pub struct AddRequest {
    /// Slug, id or URL of a mod on a platform, or path to a local jar.
    pub mod_ref: String,
    pub version: Option<String>,
    /// Mode the mod is for, alone; `None` puts it in every mode.
    pub mode: Option<String>,
    /// Direct URL of a jar no platform knows.
    pub url: Option<String>,
    /// Add the missing required dependencies without asking.
    pub with_deps: bool,
}

/// Adds a mod, then deals with its missing required dependencies. Returns the
/// key of the mod.
pub fn add(
    root: &Path,
    sources: Sources<'_>,
    prompt: &Prompt,
    req: &AddRequest,
    out: &mut dyn Write,
) -> Result<String> {
    let mode = req.mode.as_deref().filter(|m| *m != DEFAULT_MODE);
    if let Some(mode) = mode {
        modes_edit::require_mode(root, mode)?;
    }
    let mut manifest = load_or_new(root)?;

    let local = Path::new(&req.mod_ref);
    let existing = if req.with_deps && req.version.is_none() && !local.is_file() {
        in_pack(sources, &manifest, &req.mod_ref)
    } else {
        None
    };
    let key = if let Some(key) = &existing {
        key.clone()
    } else if local.is_file() {
        if req.version.is_some() {
            return Err(Error::usage(
                "--version only applies to a mod from a platform",
                "drop --version: a local jar is added as it is",
            ));
        }
        add_local(root, sources, &mut manifest, local, req.url.clone(), out)?
    } else if req.mod_ref.to_ascii_lowercase().ends_with(".jar") && !req.mod_ref.contains("://") {
        return Err(Error::usage(
            format!("no jar at {}", req.mod_ref),
            "check the path to the jar",
        ));
    } else {
        let choice = Choice {
            version: req.version.as_deref(),
            prompt: Some(prompt),
        };
        add_remote(
            root,
            sources,
            &mut manifest,
            None,
            &req.mod_ref,
            choice,
            out,
        )?
    };
    manifest.save(&root.join(manifest::FILE_NAME))?;

    let mut added = vec![key.clone()];
    add_dependencies(
        root,
        sources,
        prompt,
        req.with_deps,
        &mut manifest,
        &mut added,
        out,
    )?;
    manifest.save(&root.join(manifest::FILE_NAME))?;
    if existing.is_some() {
        // The mod was already there: only its dependencies are new.
        added.remove(0);
        if added.is_empty() {
            writeln!(
                out,
                "`{key}` is already in the pack, with every required dependency"
            )?;
        }
    }

    if let Some(mode) = mode.filter(|_| !added.is_empty()) {
        modes_edit::edit(root, |doc| {
            added
                .iter()
                .try_for_each(|k| modes_edit::move_to_mode(doc, k, mode))
        })?;
        writeln!(
            out,
            "mode `{mode}`: {} removed from the default mode and added to `{mode}`",
            quoted(&added)
        )?;
    }
    Ok(key)
}

/// Removes a mod: its jar, its entry, and its key from every mode.
pub fn remove(root: &Path, key: &str, out: &mut dyn Write) -> Result<()> {
    let path = root.join(manifest::FILE_NAME);
    let mut manifest = Manifest::load(&path)?;
    let entry = manifest.get(key).ok_or_else(|| {
        Error::usage(
            format!("no mod `{key}` in {}", manifest::FILE_NAME),
            format!("use a key listed in {}", manifest::FILE_NAME),
        )
    })?;
    if entry.is_local() {
        remove_jar(root, entry)?;
    } else if let Ok(jar) = jar_path(root, entry)
        && jar.is_file()
    {
        fs::remove_file(&jar).context(format!("removing {}", jar.display()))?;
    }
    manifest.remove(key);
    manifest.save(&path)?;
    let in_modes = modes_edit::edit(root, |doc| Ok(modes_edit::remove_key(doc, key)))?;
    writeln!(out, "removed `{key}`")?;
    if in_modes {
        writeln!(out, "removed `{key}` from the modes that listed it")?;
    }
    Ok(())
}

/// Key of the mod `query` names when the manifest already holds it: by key,
/// else by project on the platform that resolves it. A failed lookup counts
/// as not held: adding it then reports the failure.
fn in_pack(sources: Sources<'_>, manifest: &Manifest, query: &str) -> Option<String> {
    if manifest.get(query).is_some() {
        return Some(query.to_owned());
    }
    let (platform, candidates) = resolve(sources, None, query, manifest).ok()?;
    let section = &candidates.first()?.section;
    manifest
        .mods
        .iter()
        .find(|e| same_project(e.platform(platform.name()), section))
        .map(|e| e.key.clone())
}

/// How to pick a version among several.
#[derive(Clone, Copy)]
struct Choice<'a> {
    /// What `--version` names.
    version: Option<&'a str>,
    /// Asks when several versions fit and none is named; `None` takes the
    /// first, which the platform ranks best.
    prompt: Option<&'a Prompt>,
}

/// Copies a local jar into `mods/` (unless it is already there) and builds
/// its entry from what the platforms know about it.
fn add_local(
    root: &Path,
    sources: Sources<'_>,
    manifest: &mut Manifest,
    source: &Path,
    url: Option<String>,
    out: &mut dyn Write,
) -> Result<String> {
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::usage("the jar has no usable file name", "rename the jar"))?;
    if in_local_dir(root, source)? {
        return add_tracked(root, sources, manifest, name, out);
    }
    let rel = format!("{MODS_DIR}/{name}");
    let dest = root.join(MODS_DIR).join(name);
    let same_file = dest.is_file()
        && fs::canonicalize(source).context("reading the jar")?
            == fs::canonicalize(&dest).context(format!("reading {}", dest.display()))?;
    if !same_file {
        if dest.is_file()
            && hash_file(&dest)
                .context(format!("reading {}", dest.display()))?
                .sha512
                != hash_file(source).context("reading the jar")?.sha512
        {
            return Err(Error::usage(
                format!("{rel} already exists with other content"),
                "remove that mod first (`modpack remove`), or rename the jar",
            ));
        }
        fs::create_dir_all(root.join(MODS_DIR)).context("creating mods/")?;
        fs::copy(source, &dest).context(format!("copying the jar to {}", dest.display()))?;
    }

    let kept = manifest
        .mods
        .iter()
        .find(|e| e.path == rel)
        .map(|e| e.key.clone());
    let mut drafts = [Draft::from_jar(root, &rel, kept)?];
    let warnings = complete(root, &mut drafts, sources.registry);
    let [mut draft] = drafts;
    let platforms = names(&draft.entry);
    draft.entry.url = url.clone();
    let key = draft.commit(manifest, sources.registry);

    for warning in warnings {
        writeln!(out, "warning: {warning}")?;
    }
    if platforms.is_empty() {
        writeln!(out, "added `{key}` ({rel}), found on no platform")?;
        if url.is_none() {
            writeln!(
                out,
                "warning: `{key}` has no source to download it from on another machine; give its direct URL with `modpack add {rel} --url <url>`"
            )?;
        }
    } else {
        writeln!(out, "added `{key}` ({rel}), on {}", platforms.join(", "))?;
    }
    Ok(key)
}

/// Whether `jar` sits directly in the repository's `mods-local/`.
fn in_local_dir(root: &Path, jar: &Path) -> Result<bool> {
    let dir = root.join(LOCAL_DIR);
    let (Some(parent), true) = (jar.parent(), dir.is_dir()) else {
        return Ok(false);
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    Ok(fs::canonicalize(parent).context("reading the jar")?
        == fs::canonicalize(&dir).context(format!("reading {}", dir.display()))?)
}

/// Builds the entry of a local mod, whose jar `name` is in `mods-local/`. No
/// platform is asked: git holds the jar.
fn add_tracked(
    root: &Path,
    sources: Sources<'_>,
    manifest: &mut Manifest,
    name: &str,
    out: &mut dyn Write,
) -> Result<String> {
    let rel = format!("{LOCAL_DIR}/{name}");
    let kept = manifest
        .mods
        .iter()
        .find(|e| e.path == rel)
        .map(|e| e.key.clone());
    let key = Draft::from_jar(root, &rel, kept)?.commit(manifest, sources.registry);
    writeln!(
        out,
        "added the local mod `{key}` ({rel}), tracked by git; review it, then run `modpack approve {key}` before publishing"
    )?;
    Ok(key)
}

/// Resolves a mod on `on`, or on the first platform of the registry that
/// knows it, downloads the chosen version into `mods/` and builds its entry.
/// A mod already in the manifest from the same project is replaced under its
/// key, and so is a local mod whose key is the project's slug or `query`: its
/// jar leaves `mods-local/` and git.
fn add_remote(
    root: &Path,
    sources: Sources<'_>,
    manifest: &mut Manifest,
    on: Option<&dyn Platform>,
    query: &str,
    choice: Choice<'_>,
    out: &mut dyn Write,
) -> Result<String> {
    let (platform, candidates) = resolve(sources, on, query, manifest)?;
    let candidate = choose(candidates, query, &manifest.pack, choice)?;
    let name = platform.name();

    let slug_key = candidate.slug.as_deref().map(normalize_key);
    let previous = manifest
        .mods
        .iter()
        .find(|e| same_project(e.platform(name), &candidate.section))
        .or_else(|| {
            manifest.mods.iter().find(|e| {
                e.is_local() && (e.key == query || Some(e.key.as_str()) == slug_key.as_deref())
            })
        })
        .cloned();
    let rel = format!("{MODS_DIR}/{}", candidate.file_name);
    if let Some(other) = manifest
        .mods
        .iter()
        .find(|e| e.path == rel && Some(&e.key) != previous.as_ref().map(|p| &p.key))
    {
        return Err(Error::usage(
            format!("{rel} is already the jar of `{}`", other.key),
            format!("remove `{}` first with `modpack remove`", other.key),
        ));
    }

    let mut entry = Entry {
        key: previous
            .as_ref()
            .map_or_else(|| candidate.file_name.clone(), |p| p.key.clone()),
        path: rel.clone(),
        file_size: 0,
        downloads: Vec::new(),
        url: None,
        source: None,
        approved: None,
        hashes: Hashes {
            sha1: String::new(),
            sha512: String::new(),
        },
        env: Env::default(),
        platforms: BTreeMap::from([(name.to_owned(), candidate.section.clone())]),
    };
    platform.fill_entry(&mut entry)?;
    fetch(root, sources, platform, &mut entry)?;
    let replaced_local = previous.as_ref().filter(|p| p.is_local()).cloned();
    if let Some(local) = &replaced_local {
        remove_jar(root, local)?;
    } else if let Some(previous) = &previous
        && previous.path != entry.path
        && let Ok(old) = jar_path(root, previous)
        && old.is_file()
    {
        fs::remove_file(&old).context(format!("removing {}", old.display()))?;
    }

    let mut drafts = [Draft {
        entry,
        key: previous.map(|p| p.key),
        slugs: BTreeMap::from([(name.to_owned(), candidate.slug.clone())]),
    }];
    let warnings = complete(root, &mut drafts, sources.registry);
    let [draft] = drafts;
    let platforms = names(&draft.entry);
    let key = draft.commit(manifest, sources.registry);
    for warning in warnings {
        writeln!(out, "warning: {warning}")?;
    }
    writeln!(
        out,
        "added `{key}` {} ({rel}), on {}",
        candidate.label,
        platforms.join(", ")
    )?;
    if let Some(local) = replaced_local {
        writeln!(
            out,
            "replaced the local mod `{key}`: {} removed from mods-local/ and from git",
            local.path
        )?;
    }
    Ok(key)
}

/// The platform that knows `query` and its candidates: `on` alone when given,
/// else the first platform of the registry with a candidate.
fn resolve<'a>(
    sources: Sources<'a>,
    on: Option<&'a dyn Platform>,
    query: &str,
    manifest: &Manifest,
) -> Result<(&'a dyn Platform, Vec<Candidate>)> {
    let platforms: Vec<&dyn Platform> = match on {
        Some(p) => vec![p],
        None => sources.registry.all().collect(),
    };
    let mut failure = None;
    for platform in platforms {
        match platform.resolve(query, &manifest.pack) {
            Ok(candidates) if !candidates.is_empty() => return Ok((platform, candidates)),
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(platform = platform.name(), "resolve failed");
                failure.get_or_insert(err);
            }
        }
    }
    Err(failure.unwrap_or_else(|| {
        Error::usage(
            format!(
                "no version of `{query}` for Minecraft {} with {}",
                manifest.pack.minecraft, manifest.pack.loader
            ),
            "check the slug or URL, or add a local jar",
        )
    }))
}

/// How many versions an error lists at most.
const LISTED_VERSIONS: usize = 10;

fn choose(
    mut candidates: Vec<Candidate>,
    query: &str,
    pack: &Pack,
    choice: Choice<'_>,
) -> Result<Candidate> {
    if let Some(wanted) = choice.version {
        return candidates
            .iter()
            .position(|c| names_version(c, wanted))
            .map(|i| candidates.swap_remove(i))
            .ok_or_else(|| {
                let labels: Vec<&str> = candidates
                    .iter()
                    .take(LISTED_VERSIONS)
                    .map(|c| c.label.as_str())
                    .collect();
                Error::usage(
                    format!("`{query}` has no version `{wanted}` for this pack"),
                    format!("pass one of these to --version: {}", labels.join(", ")),
                )
            });
    }
    let prompt = match choice.prompt {
        Some(prompt) if candidates.len() > 1 => prompt,
        _ => return Ok(candidates.swap_remove(0)),
    };
    if !prompt.is_interactive() {
        return Err(several_versions(&candidates, query, pack));
    }
    let labels: Vec<String> = candidates.iter().map(|c| c.label.clone()).collect();
    let picked = prompt.select(None, "--version", &format!("Version of {query}"), &labels)?;
    let index = labels.iter().position(|l| *l == picked).unwrap_or(0);
    Ok(candidates.swap_remove(index))
}

/// Several versions fit and there is no terminal to ask: lists them, in the
/// platform's order (newest first), and the argument that picks one.
fn several_versions(candidates: &[Candidate], query: &str, pack: &Pack) -> Error {
    let mut what = format!(
        "`{query}` has {} versions for Minecraft {} with {}, and no terminal to ask which one (newest first):",
        candidates.len(),
        pack.minecraft,
        pack.loader
    );
    for candidate in candidates.iter().take(LISTED_VERSIONS) {
        what.push_str(&format!("\n  - {}", version_arg(candidate)));
    }
    if candidates.len() > LISTED_VERSIONS {
        what.push_str(&format!(
            "\n  - and {} older",
            candidates.len() - LISTED_VERSIONS
        ));
    }
    Error::usage(
        what,
        format!(
            "pass the version with --version, e.g. `modpack add {query} --version {}`",
            version_arg(&candidates[0])
        ),
    )
}

/// What to pass to `--version` for a candidate: its label up to a
/// parenthesis, quoted for the shell when it holds a space.
fn version_arg(candidate: &Candidate) -> String {
    let name = candidate
        .label
        .split(" (")
        .next()
        .unwrap_or(&candidate.label);
    if name.contains(char::is_whitespace) {
        format!("'{}'", name.replace('\'', "'\\''"))
    } else {
        name.to_owned()
    }
}

/// Whether `wanted` names the candidate: its label, the name before a
/// parenthesis in its label, its file name, or one of its platform ids.
fn names_version(candidate: &Candidate, wanted: &str) -> bool {
    candidate.label == wanted
        || candidate.label.split(" (").next() == Some(wanted)
        || candidate.file_name == wanted
        || candidate
            .section
            .values()
            .any(|v| id_text(v).as_deref() == Some(wanted))
}

/// Downloads the jar of a new entry. With hashes known, through the stock,
/// which checks them; without, straight from the platform, and the hashes are
/// taken from what arrived.
fn fetch(
    root: &Path,
    sources: Sources<'_>,
    platform: &dyn Platform,
    entry: &mut Entry,
) -> Result<()> {
    if !entry.hashes.sha512.is_empty() {
        return download(root, entry, sources).map_err(|failure| {
            let causes: Vec<String> = failure
                .attempts
                .iter()
                .map(|a| format!("{}: {}", a.source, first_line(&a.error)))
                .collect();
            Error::internal(
                format!("cannot download `{}` ({})", entry.key, causes.join("; ")),
                "check the network connection and retry",
            )
        });
    }
    let dest = jar_path(root, entry)?;
    let mut reader = match platform.download(entry)? {
        Download::Url(url) => sources.fetch.get(&url)?,
        Download::Stream(stream) => stream,
    };
    let hashes = write_atomic(&dest, |file| {
        copy_hashing(&mut reader, file).context("receiving the jar")
    })?;
    entry.hashes = Hashes {
        sha1: hashes.sha1,
        sha512: hashes.sha512,
    };
    entry.file_size = hashes.size;
    Ok(())
}

/// Lists the required dependencies of the mods in `added` that no mod of the
/// manifest provides, and adds them with `--with-deps` or when the user
/// agrees. The mods added this way are appended to `added`.
fn add_dependencies(
    root: &Path,
    sources: Sources<'_>,
    prompt: &Prompt,
    with_deps: bool,
    manifest: &mut Manifest,
    added: &mut Vec<String>,
    out: &mut dyn Write,
) -> Result<()> {
    let mut missing = missing_dependencies(manifest, added);
    if missing.is_empty() {
        return Ok(());
    }
    for dep in &mut missing {
        dep.name = name_of(sources, dep);
    }
    writeln!(out, "required dependencies missing from the pack:")?;
    for dep in &missing {
        writeln!(out, "  - {dep}")?;
    }
    let go = with_deps
        || (prompt.is_interactive() && prompt.confirm(None, "--with-deps", "Add them?", true)?);
    if !go {
        writeln!(
            out,
            "add them with `modpack add {} --with-deps`, or one by one with `modpack add`",
            added[0]
        )?;
        return Ok(());
    }

    let mut tried = BTreeSet::new();
    loop {
        let Some(dep) = missing_dependencies(manifest, added)
            .into_iter()
            .find(|d| !tried.contains(d))
        else {
            return Ok(());
        };
        tried.insert(dep.clone());
        let Some(platform) = sources.registry.find(&dep.platform) else {
            continue;
        };
        let choice = Choice {
            version: None,
            prompt: None,
        };
        match add_remote(
            root,
            sources,
            manifest,
            Some(platform),
            &dep.id,
            choice,
            out,
        ) {
            Ok(key) => added.push(key),
            Err(err) => writeln!(out, "warning: could not add {dep}: {}", first_line(&err))?,
        }
    }
}

/// Slug and title of a dependency, from its platform. A failed lookup (no
/// network, no API key) leaves the dependency named by its id.
fn name_of(sources: Sources<'_>, dep: &MissingDependency) -> Option<ProjectName> {
    let platform = sources.registry.find(&dep.platform)?;
    match platform.project_name(&dep.id) {
        Ok(name) => name,
        Err(err) => {
            tracing::debug!(platform = dep.platform, id = dep.id, "{}", first_line(&err));
            None
        }
    }
}

/// A required dependency no mod of the manifest provides.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MissingDependency {
    pub platform: String,
    /// Project id on the platform.
    pub id: String,
    /// Key of the mod that needs it.
    pub needed_by: String,
    /// How the platform names the project, when it was asked.
    pub name: Option<ProjectName>,
}

impl std::fmt::Display for MissingDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.name {
            Some(name) => write!(
                f,
                "`{}` ({}, {} id `{}`), needed by `{}`",
                name.slug, name.title, self.platform, self.id, self.needed_by
            ),
            None => write!(
                f,
                "{} project `{}`, needed by `{}`",
                self.platform, self.id, self.needed_by
            ),
        }
    }
}

/// The required dependencies of the mods `keys` that no entry of the manifest
/// provides on the same platform. A dependency listed by several mods, or on
/// several platforms, appears once per platform and id.
pub fn missing_dependencies(manifest: &Manifest, keys: &[String]) -> Vec<MissingDependency> {
    let provided: BTreeSet<(&str, String)> = manifest
        .mods
        .iter()
        .flat_map(|e| e.platforms.iter())
        .filter_map(|(platform, table)| Some((platform.as_str(), project_id(table)?)))
        .collect();
    let mut seen = BTreeSet::new();
    let mut missing = Vec::new();
    for entry in keys.iter().filter_map(|k| manifest.get(k)) {
        for (platform, table) in &entry.platforms {
            let deps = table.get("dependencies").and_then(toml::Value::as_array);
            for id in deps.into_iter().flatten().filter_map(id_text) {
                if provided.contains(&(platform.as_str(), id.clone()))
                    || !seen.insert((platform.clone(), id.clone()))
                {
                    continue;
                }
                missing.push(MissingDependency {
                    platform: platform.clone(),
                    id,
                    needed_by: entry.key.clone(),
                    name: None,
                });
            }
        }
    }
    missing
}

fn project_id(table: &toml::Table) -> Option<String> {
    PROJECT_ID_FIELDS
        .iter()
        .find_map(|f| table.get(*f).and_then(id_text))
}

/// Whether a platform table names the project of `section`.
fn same_project(table: Option<&toml::Table>, section: &toml::Table) -> bool {
    match (table.and_then(project_id), project_id(section)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// A project id as text: Modrinth writes strings, CurseForge integers.
fn id_text(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(n) => Some(n.to_string()),
        _ => None,
    }
}

fn names(entry: &Entry) -> Vec<String> {
    entry.platforms.keys().cloned().collect()
}

fn quoted(keys: &[String]) -> String {
    keys.iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::ops::manifest::tests::{registry, repo};
    use crate::platform::fake::{FakeMod, FakePlatform};
    use crate::platform::{Capability, Operation, Registry};
    use crate::store::download::tests::NoFetch;

    fn sources(registry: &Registry) -> Sources<'_> {
        Sources {
            registry,
            fetch: &NoFetch,
        }
    }

    fn request(mod_ref: &str) -> AddRequest {
        AddRequest {
            mod_ref: mod_ref.into(),
            ..Default::default()
        }
    }

    fn run_add(root: &Path, registry: &Registry, req: &AddRequest) -> (Result<String>, String) {
        let mut out = Vec::new();
        let result = add(root, sources(registry), &Prompt::new(false), req, &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    fn load(root: &Path) -> Manifest {
        Manifest::load(&root.join(manifest::FILE_NAME)).unwrap()
    }

    const MODES: &str = "# Modes, written by hand.\n[default]\ndescription = \"Full\"\n\n# The server.\n[modes.server]\nremove = [\"jei\"]\n";

    #[test]
    fn remote_mod_is_downloaded_and_gets_both_sets_of_ids() {
        let root = repo();
        let (result, out) = run_add(root.path(), &registry(), &request("jei"));
        assert_eq!(result.unwrap(), "jei");
        assert!(out.contains("added `jei`"), "{out}");
        assert_eq!(
            fs::read(root.path().join("mods/jei-1.0.0.jar")).unwrap(),
            b"jei"
        );
        let manifest = load(root.path());
        let jei = manifest.get("jei").unwrap();
        assert_eq!(jei.platform("fake").unwrap()["id"].as_str(), Some("jei"));
        assert_eq!(
            jei.platform("other").unwrap()["id"].as_str(),
            Some("just-enough-items")
        );
        assert_eq!(jei.file_size, 3);
        assert_eq!(jei.downloads.len(), 1);
        assert!(crate::ops::manifest::check(root.path()).unwrap().is_empty());
    }

    #[test]
    fn local_jar_found_on_a_platform_is_identified() {
        let root = repo();
        let elsewhere = tempfile::tempdir().unwrap();
        let jar = elsewhere.path().join("sodium-1.0.0.jar");
        fs::write(&jar, b"sodium").unwrap();
        let (result, out) = run_add(root.path(), &registry(), &request(jar.to_str().unwrap()));
        assert_eq!(result.unwrap(), "sodium");
        assert!(out.contains("on fake"), "{out}");
        assert!(root.path().join("mods/sodium-1.0.0.jar").is_file());
        let manifest = load(root.path());
        let sodium = manifest.get("sodium").unwrap();
        assert!(sodium.platform("fake").is_some());
        assert!(sodium.platform("other").is_none());
    }

    #[test]
    fn local_jar_found_nowhere_asks_for_a_url() {
        let root = repo();
        let elsewhere = tempfile::tempdir().unwrap();
        let jar = elsewhere.path().join("custom-mod-1.0.jar");
        fs::write(&jar, b"custom").unwrap();
        let (result, out) = run_add(root.path(), &registry(), &request(jar.to_str().unwrap()));
        assert_eq!(result.unwrap(), "custom-mod");
        assert!(out.contains("found on no platform"), "{out}");
        assert!(out.contains("--url"), "{out}");
        let entry = load(root.path()).get("custom-mod").unwrap().clone();
        assert!(entry.platforms.is_empty() && entry.url.is_none());

        // Adding the jar already in mods/ again, with its URL, keeps the key.
        let mut req = request(
            root.path()
                .join("mods/custom-mod-1.0.jar")
                .to_str()
                .unwrap(),
        );
        req.url = Some("https://example.org/custom-mod-1.0.jar".into());
        let (result, out) = run_add(root.path(), &registry(), &req);
        assert_eq!(result.unwrap(), "custom-mod");
        assert!(!out.contains("warning"), "{out}");
        let manifest = load(root.path());
        assert_eq!(manifest.mods.len(), 1);
        assert_eq!(
            manifest.get("custom-mod").unwrap().url.as_deref(),
            Some("https://example.org/custom-mod-1.0.jar")
        );
    }

    #[test]
    fn mode_moves_the_mod_out_of_the_default_mode() {
        let root = repo();
        fs::write(root.path().join("modes.toml"), MODES).unwrap();
        let mut req = request("sodium");
        req.mode = Some("server".into());
        let (result, out) = run_add(root.path(), &registry(), &req);
        result.unwrap();
        assert!(out.contains("added to `server`"), "{out}");
        let modes = fs::read_to_string(root.path().join("modes.toml")).unwrap();
        assert_eq!(
            modes,
            "# Modes, written by hand.\n[default]\ndescription = \"Full\"\nremove = [\"sodium\"]\n\n# The server.\n[modes.server]\nremove = [\"jei\"]\nadd = [\"sodium\"]\n"
        );
    }

    #[test]
    fn unknown_mode_is_refused_before_anything_is_downloaded() {
        let root = repo();
        fs::write(root.path().join("modes.toml"), MODES).unwrap();
        let mut req = request("sodium");
        req.mode = Some("lowend".into());
        let err = run_add(root.path(), &registry(), &req).0.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(!root.path().join("mods/sodium-1.0.0.jar").exists());
        assert!(!root.path().join(manifest::FILE_NAME).exists());
    }

    /// `sodium` needs `fabric-api`, which needs `fabric-loader-lib`.
    fn dependency_chain() -> FakePlatform {
        let mut sodium = FakeMod::new("sodium");
        sodium.dependencies = vec!["fabric-api".into()];
        let mut fabric = FakeMod::new("fabric-api");
        fabric.dependencies = vec!["fabric-loader-lib".into()];
        FakePlatform::new("fake")
            .with(sodium)
            .with(fabric)
            .with(FakeMod::new("fabric-loader-lib"))
    }

    fn with_dependency() -> Registry {
        Registry::new(vec![Box::new(dependency_chain())])
    }

    #[test]
    fn missing_dependency_is_listed() {
        let root = repo();
        let (result, out) = run_add(root.path(), &with_dependency(), &request("sodium"));
        result.unwrap();
        assert!(
            out.contains(
                "missing from the pack:\n  - `fabric-api` (Title of fabric-api, fake id `fabric-api`), needed by `sodium`\n"
            ),
            "{out}"
        );
        assert!(out.contains("`modpack add sodium --with-deps`"), "{out}");
        assert!(load(root.path()).get("fabric-api").is_none());
    }

    #[test]
    fn dependency_the_platform_cannot_name_keeps_its_id() {
        let root = repo();
        let mut sodium = FakeMod::new("sodium");
        sodium.dependencies = vec!["eXts2L7r".into()];
        let registry = Registry::new(vec![Box::new(FakePlatform::new("fake").with(sodium))]);
        let (result, out) = run_add(root.path(), &registry, &request("sodium"));
        result.unwrap();
        assert!(
            out.contains("  - fake project `eXts2L7r`, needed by `sodium`\n"),
            "{out}"
        );
    }

    #[test]
    fn with_deps_adds_dependencies_and_theirs_to_the_same_mode() {
        let root = repo();
        fs::write(root.path().join("modes.toml"), MODES).unwrap();
        let mut req = request("sodium");
        req.with_deps = true;
        req.mode = Some("server".into());
        let (result, out) = run_add(root.path(), &with_dependency(), &req);
        result.unwrap();
        let manifest = load(root.path());
        assert_eq!(
            manifest.keys().into_iter().collect::<Vec<_>>(),
            ["fabric-api", "fabric-loader-lib", "sodium"]
        );
        assert!(missing_dependencies(&manifest, &["sodium".into()]).is_empty());
        let modes = fs::read_to_string(root.path().join("modes.toml")).unwrap();
        assert!(
            modes.contains("add = [\"sodium\", \"fabric-api\", \"fabric-loader-lib\"]"),
            "{modes}\n{out}"
        );
    }

    #[test]
    fn with_deps_on_a_mod_in_the_pack_adds_only_its_dependencies() {
        let root = repo();
        fs::write(root.path().join("modes.toml"), MODES).unwrap();
        let registry = Registry::new(vec![Box::new(TwoVersions(dependency_chain()))]);
        let mut req = request("sodium");
        req.version = Some("0.9.0".into());
        run_add(root.path(), &registry, &req).0.unwrap();

        // No --version although two versions fit, and no second "added `sodium`".
        let mut req = request("sodium");
        req.with_deps = true;
        let (result, out) = run_add(root.path(), &registry, &req);
        assert_eq!(result.unwrap(), "sodium");
        assert!(!out.contains("added `sodium`"), "{out}");
        assert!(out.contains("added `fabric-api`"), "{out}");
        assert!(out.contains("added `fabric-loader-lib`"), "{out}");
        assert!(root.path().join("mods/sodium-1.0.0.jar").is_file());

        let (result, out) = run_add(root.path(), &registry, &req);
        result.unwrap();
        assert_eq!(
            out,
            "`sodium` is already in the pack, with every required dependency\n"
        );

        // Found by its project when the key is not the slug.
        let path = root.path().join(manifest::FILE_NAME);
        let mut manifest = load(root.path());
        let mut renamed = manifest.remove("sodium").unwrap();
        renamed.key = "rubidium".into();
        manifest.upsert(renamed);
        manifest.save(&path).unwrap();
        let (result, out) = run_add(root.path(), &registry, &req);
        assert_eq!(result.unwrap(), "rubidium");
        assert!(
            out.starts_with("`rubidium` is already in the pack"),
            "{out}"
        );
    }

    /// Serves two versions of every mod `fake` knows.
    struct TwoVersions(FakePlatform);

    impl Platform for TwoVersions {
        fn name(&self) -> &str {
            self.0.name()
        }
        fn capability(&self, op: Operation) -> Capability {
            self.0.capability(op)
        }
        fn resolve(
            &self,
            query: &str,
            pack: &crate::model::manifest::Pack,
        ) -> Result<Vec<Candidate>> {
            let mut candidates = self.0.resolve(query, pack)?;
            if let Some(first) = candidates.first().cloned() {
                let mut older = first;
                older.label = "0.9.0".into();
                candidates.push(older);
            }
            Ok(candidates)
        }
        fn fill_entry(&self, entry: &mut Entry) -> Result<()> {
            self.0.fill_entry(entry)
        }
        fn download(&self, entry: &Entry) -> Result<Download> {
            self.0.download(entry)
        }
    }

    #[test]
    fn several_versions_without_terminal_name_the_version_argument() {
        let root = repo();
        let registry = Registry::new(vec![Box::new(TwoVersions(
            FakePlatform::new("fake").with(FakeMod::new("sodium")),
        ))]);
        let err = run_add(root.path(), &registry, &request("sodium"))
            .0
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        let text = err.to_string();
        assert!(
            text.contains("`sodium` has 2 versions for Minecraft 1.21.1 with neoforge"),
            "{text}"
        );
        assert!(
            text.contains("\n  - sodium-1.0.0.jar\n  - 0.9.0\n"),
            "{text}"
        );
        assert_eq!(
            err.hint(),
            "pass the version with --version, e.g. `modpack add sodium --version sodium-1.0.0.jar`"
        );

        let mut req = request("sodium");
        req.version = Some("0.9.0".into());
        let (result, out) = run_add(root.path(), &registry, &req);
        result.unwrap();
        assert!(out.contains("`sodium` 0.9.0"), "{out}");

        req.version = Some("2.0".into());
        let err = run_add(root.path(), &registry, &req).0.unwrap_err();
        assert!(
            err.hint().contains("sodium-1.0.0.jar, 0.9.0"),
            "{}",
            err.hint()
        );
    }

    #[test]
    fn version_argument_drops_the_details_and_quotes_spaces() {
        let candidate = |label: &str| Candidate {
            label: label.into(),
            slug: None,
            file_name: "a.jar".into(),
            section: toml::Table::new(),
        };
        assert_eq!(version_arg(&candidate("0.6.0+mc1.21.1")), "0.6.0+mc1.21.1");
        assert_eq!(
            version_arg(&candidate("Jade 15.1 (release, 2024-08-01)")),
            "'Jade 15.1'"
        );
    }

    #[test]
    fn unknown_mod_is_a_usage_error() {
        let root = repo();
        let err = run_add(root.path(), &registry(), &request("nope"))
            .0
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("no version of `nope`"));
        let err = run_add(root.path(), &registry(), &request("missing.jar"))
            .0
            .unwrap_err();
        assert!(err.to_string().contains("no jar at missing.jar"));
    }

    #[test]
    fn adding_the_same_project_again_replaces_it_under_its_key() {
        let root = repo();
        run_add(root.path(), &registry(), &request("sodium"))
            .0
            .unwrap();
        let path = root.path().join(manifest::FILE_NAME);
        let mut manifest = load(root.path());
        let mut renamed = manifest.remove("sodium").unwrap();
        renamed.key = "rubidium".into();
        manifest.upsert(renamed);
        manifest.save(&path).unwrap();

        let key = run_add(root.path(), &registry(), &request("sodium"))
            .0
            .unwrap();
        assert_eq!(key, "rubidium");
        assert_eq!(load(root.path()).mods.len(), 1);
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// A git repository with the local mod `sodium` (a development build),
    /// staged, listed in the manifest and in a mode.
    fn with_local_sodium() -> tempfile::TempDir {
        let root = repo();
        git(root.path(), &["init", "--quiet"]);
        fs::create_dir_all(root.path().join("mods-local")).unwrap();
        fs::write(root.path().join("mods-local/sodium-dev.jar"), b"dev build").unwrap();
        fs::write(root.path().join("modes.toml"), MODES).unwrap();
        let jar = root.path().join("mods-local/sodium-dev.jar");
        let (result, out) = run_add(root.path(), &registry(), &request(jar.to_str().unwrap()));
        assert_eq!(result.unwrap(), "sodium-dev");
        assert!(out.contains("added the local mod `sodium-dev`"), "{out}");
        assert!(out.contains("modpack approve sodium-dev"), "{out}");
        // Renamed to the slug the published mod will have.
        let path = root.path().join(manifest::FILE_NAME);
        let mut manifest = load(root.path());
        let mut entry = manifest.remove("sodium-dev").unwrap();
        entry.key = "sodium".into();
        manifest.upsert(entry);
        manifest.save(&path).unwrap();
        git(root.path(), &["add", "mods-local"]);
        root
    }

    #[test]
    fn a_jar_of_mods_local_is_added_as_a_local_mod() {
        let root = with_local_sodium();
        let sodium = load(root.path()).get("sodium").unwrap().clone();
        assert!(sodium.is_local());
        assert_eq!(sodium.path, "mods-local/sodium-dev.jar");
        assert!(sodium.platforms.is_empty());
        assert!(!root.path().join("mods/sodium-dev.jar").exists());
        assert!(crate::ops::manifest::check(root.path()).unwrap().is_empty());
    }

    #[test]
    fn adding_the_published_mod_replaces_the_local_one_and_its_jar_leaves_git() {
        let root = with_local_sodium();
        let modes = fs::read_to_string(root.path().join("modes.toml")).unwrap();

        let (result, out) = run_add(root.path(), &registry(), &request("sodium"));
        assert_eq!(result.unwrap(), "sodium");
        assert!(out.contains("replaced the local mod `sodium`"), "{out}");
        let manifest = load(root.path());
        assert_eq!(manifest.mods.len(), 1);
        let sodium = manifest.get("sodium").unwrap();
        assert!(!sodium.is_local() && sodium.approved.is_none());
        assert_eq!(sodium.path, "mods/sodium-1.0.0.jar");
        assert!(sodium.platform("fake").is_some());
        assert!(!root.path().join("mods-local/sodium-dev.jar").exists());
        assert_eq!(git(root.path(), &["ls-files", "mods-local"]), "");
        assert_eq!(
            fs::read_to_string(root.path().join("modes.toml")).unwrap(),
            modes
        );
    }

    #[test]
    fn removing_a_local_mod_takes_its_jar_out_of_git() {
        let root = with_local_sodium();
        remove(root.path(), "sodium", &mut Vec::new()).unwrap();
        assert!(!root.path().join("mods-local/sodium-dev.jar").exists());
        assert_eq!(git(root.path(), &["ls-files", "mods-local"]), "");
        assert!(load(root.path()).mods.is_empty());
    }

    #[test]
    fn remove_cleans_manifest_mods_and_modes() {
        let root = repo();
        let modes = "# Hand-written.\n[default]\nremove = [\"sodium\", \"jei\"]\n\n[modes.server]\n# Server only.\nadd = [\"sodium\"]\n";
        fs::write(root.path().join("modes.toml"), modes).unwrap();
        run_add(root.path(), &registry(), &request("sodium"))
            .0
            .unwrap();
        run_add(root.path(), &registry(), &request("jei"))
            .0
            .unwrap();

        let mut out = Vec::new();
        remove(root.path(), "sodium", &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("from the modes that listed it"), "{out}");
        assert!(!root.path().join("mods/sodium-1.0.0.jar").exists());
        assert!(root.path().join("mods/jei-1.0.0.jar").exists());
        assert_eq!(
            load(root.path()).keys().into_iter().collect::<Vec<_>>(),
            ["jei"]
        );
        assert_eq!(
            fs::read_to_string(root.path().join("modes.toml")).unwrap(),
            "# Hand-written.\n[default]\nremove = [\"jei\"]\n\n[modes.server]\n# Server only.\nadd = []\n"
        );
        assert!(crate::ops::manifest::check(root.path()).unwrap().is_empty());

        let err = remove(root.path(), "sodium", &mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
    }
}
