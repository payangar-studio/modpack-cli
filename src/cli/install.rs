//! `modpack install`: downloads the missing jars into mods/, then reloads the
//! mode loaded in the pack's instance.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::context::Context;
use crate::error::{Context as _, Error, Result};
use crate::launcher::running::{GameDetector, Processes};
use crate::mode::{self, LoadContext};
use crate::model::installed::Installed;
use crate::model::instance::{InstanceState, Instances};
use crate::model::manifest::{self, Entry, Manifest};
use crate::model::socle::SocleConfig;
use crate::platform::Registry;
use crate::store::download::{Failure, Http, Sources, download_all};
use crate::store::inventory::{Inventory, inventory};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Replace the jars whose hash differs from the manifest
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let registry = Registry::builtin();
    let http = Http::new();
    let sources = Sources {
        registry: &registry,
        fetch: &http,
    };
    let instances = Instances::default_path()?;
    install(
        root,
        sources,
        args.force,
        &instances,
        &Processes,
        &mut std::io::stdout().lock(),
    )
}

/// The command, with its sources and the machine's instances file given.
/// Also step 2 of `modpack setup`.
pub(crate) fn install(
    root: &Path,
    sources: Sources<'_>,
    force: bool,
    instances_file: &Path,
    game: &dyn GameDetector,
    out: &mut dyn Write,
) -> Result<()> {
    let manifest_path = root.join(manifest::FILE_NAME);
    if !manifest_path.is_file() {
        return Err(Error::usage(
            format!("no {} in this repository", manifest::FILE_NAME),
            "generate it with `modpack manifest`",
        ));
    }
    let manifest = Manifest::load(&manifest_path)?;
    let inv = inventory(root, &manifest)?;
    remove_stale(root, &inv)?;

    let mut wanted: Vec<&Entry> = entries(&manifest, &inv.missing);
    if force {
        wanted.extend(entries(&manifest, &inv.mismatched));
    }
    let mut downloaded = 0;
    let mut failures = Vec::new();
    for result in download_all(root, &wanted, sources)? {
        match result {
            Ok(()) => downloaded += 1,
            Err(failure) => failures.push(failure),
        }
    }

    writeln!(
        out,
        "jars: {} present, {downloaded} downloaded, {} failed",
        inv.present.len(),
        failures.len()
    )?;
    for failure in &failures {
        write_failure(out, failure)?;
    }
    if !inv.stale.is_empty() {
        writeln!(
            out,
            "removed, no longer in the manifest: {}",
            inv.stale.join(", ")
        )?;
    }
    if force && !inv.mismatched.is_empty() {
        writeln!(
            out,
            "replaced, their hash differed from the manifest: {}",
            inv.mismatched.join(", ")
        )?;
    }
    if !force && !inv.mismatched.is_empty() {
        writeln!(
            out,
            "hash differs from the manifest, not replaced: {}",
            inv.mismatched.join(", ")
        )?;
    }
    if !inv.unknown.is_empty() {
        writeln!(
            out,
            "not in the manifest, left as is: {}",
            inv.unknown.join(", ")
        )?;
    }

    if !failures.is_empty() {
        return Err(Error::internal(
            format!("{} jar(s) could not be downloaded", failures.len()),
            "fix the causes listed above, then run `modpack install` again",
        ));
    }
    if !force && !inv.mismatched.is_empty() {
        return Err(Error::blocked(
            format!(
                "{} jar(s) in mods/ differ from the manifest",
                inv.mismatched.len()
            ),
            "rerun with --force to replace them with the manifest's version, or run `modpack manifest` if you changed them on purpose",
        ));
    }
    let ctx = LoadContext {
        root,
        instances_file,
        game,
        sources,
    };
    reload(&ctx, out)
}

/// Removes from `mods/` the jars the CLI downloaded that the manifest no
/// longer lists, and forgets the recorded jars that are gone or were changed
/// since: those are the user's now.
fn remove_stale(root: &Path, inv: &Inventory) -> Result<()> {
    for rel in &inv.stale {
        let path = root.join(rel);
        fs::remove_file(&path).context(format!("removing {}", path.display()))?;
    }
    let mut installed = Installed::load(root)?;
    let before = installed.jars.len();
    installed.jars.retain(|rel, sha1| {
        !inv.stale.contains(rel) && inv.jars.get(rel).is_some_and(|h| &h.sha1 == sha1)
    });
    if installed.jars.len() != before {
        installed.save(root)?;
    }
    Ok(())
}

fn entries<'a>(manifest: &'a Manifest, keys: &[String]) -> Vec<&'a Entry> {
    keys.iter().filter_map(|k| manifest.get(k)).collect()
}

/// One failed jar: every source tried, with what went wrong and what to do.
fn write_failure(out: &mut dyn Write, failure: &Failure) -> Result<()> {
    writeln!(out, "failed: {}", failure.key)?;
    for attempt in &failure.attempts {
        let text = attempt.error.to_string();
        let mut lines = text.lines();
        let first = lines.next().unwrap_or_default();
        let first = first.strip_prefix("error: ").unwrap_or(first);
        writeln!(out, "  from {}: {first}", attempt.source)?;
        for line in lines {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

/// Reloads the mode loaded in the pack's instance, if the pack has an
/// instance on this machine and a mode was loaded there.
fn reload(ctx: &LoadContext<'_>, out: &mut dyn Write) -> Result<()> {
    let name = SocleConfig::load(ctx.root)?.init.name;
    let Some(instance) = Instances::load(ctx.instances_file)?
        .get(&name)
        .filter(|dir| dir.is_dir())
        .map(Path::to_path_buf)
    else {
        tracing::info!(pack = %name, "no instance on this machine, nothing to reload");
        return Ok(());
    };
    let Some(mode) = InstanceState::load(&instance)?.mode else {
        tracing::info!(instance = %instance.display(), "no mode loaded yet, nothing to reload");
        return Ok(());
    };
    let report = mode::load(ctx, &mode, false)?;
    write!(out, "{report}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::model::socle::InitValues;
    use crate::store::download::tests::{MapFetch, NoFetch, entry, manifest};

    /// Minecraft never runs.
    struct NoGame;

    impl GameDetector for NoGame {
        fn is_running(&self, _instance_dir: &Path) -> Result<bool> {
            Ok(false)
        }
    }

    const PACK: &str = "Test Pack";

    /// A repository whose manifest lists `jei` and `iris`, with `jei` served
    /// by its download URL; and an empty machine config.
    struct Repo {
        root: tempfile::TempDir,
        machine: tempfile::TempDir,
        fetch: MapFetch,
        registry: Registry,
    }

    impl Repo {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            SocleConfig {
                version: "0.1.0".into(),
                init: InitValues {
                    name: PACK.into(),
                    minecraft: "1.21.1".into(),
                    loader: "neoforge".into(),
                    loader_version: "21.1.77".into(),
                    author: "a".into(),
                },
            }
            .save(root.path())
            .unwrap();
            let mut fetch = MapFetch::default();
            let mods = ["jei", "iris"].map(|key| {
                let mut e = entry(key, key.as_bytes());
                let url = format!("https://cdn.invalid/{key}.jar");
                fetch = std::mem::take(&mut fetch).with(&url, key.as_bytes());
                e.downloads = vec![url];
                e
            });
            manifest(mods.to_vec())
                .save(&root.path().join(manifest::FILE_NAME))
                .unwrap();
            Self {
                root,
                machine: tempfile::tempdir().unwrap(),
                fetch,
                registry: Registry::new(Vec::new()),
            }
        }

        fn path(&self, relative: &str) -> std::path::PathBuf {
            self.root.path().join(relative)
        }

        fn instances_file(&self) -> std::path::PathBuf {
            self.machine.path().join("instances.toml")
        }

        fn install_with(
            &self,
            fetch: &dyn crate::store::download::Fetch,
            force: bool,
        ) -> (Result<()>, String) {
            let mut out = Vec::new();
            let sources = Sources {
                registry: &self.registry,
                fetch,
            };
            let result = install(
                self.root.path(),
                sources,
                force,
                &self.instances_file(),
                &NoGame,
                &mut out,
            );
            (result, String::from_utf8(out).unwrap())
        }

        fn install(&self, force: bool) -> (Result<()>, String) {
            self.install_with(&self.fetch, force)
        }
    }

    #[test]
    fn downloads_the_missing_jars() {
        let repo = Repo::new();
        let (result, out) = repo.install(false);
        result.unwrap();
        assert!(
            out.contains("jars: 0 present, 2 downloaded, 0 failed"),
            "{out}"
        );
        assert_eq!(fs::read(repo.path("mods/jei-1.0.0.jar")).unwrap(), b"jei");
        assert_eq!(fs::read(repo.path("mods/iris-1.0.0.jar")).unwrap(), b"iris");
    }

    #[test]
    fn complete_stock_downloads_nothing() {
        let repo = Repo::new();
        repo.install(false).0.unwrap();
        fs::write(repo.path("mods/tried.jar"), b"x").unwrap();
        let (result, out) = repo.install_with(&NoFetch, false);
        result.unwrap();
        assert!(
            out.contains("jars: 2 present, 0 downloaded, 0 failed"),
            "{out}"
        );
        assert!(out.contains("left as is: mods/tried.jar"), "{out}");
    }

    #[test]
    fn different_hash_blocks_without_force_and_is_replaced_with_it() {
        let repo = Repo::new();
        repo.install(false).0.unwrap();
        fs::write(repo.path("mods/iris-1.0.0.jar"), b"edited").unwrap();

        let (result, out) = repo.install_with(&NoFetch, false);
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Blocked);
        assert!(out.contains("not replaced: iris"), "{out}");
        assert_eq!(
            fs::read(repo.path("mods/iris-1.0.0.jar")).unwrap(),
            b"edited"
        );

        let (result, out) = repo.install(true);
        result.unwrap();
        assert!(out.contains("jars: 1 present, 1 downloaded"), "{out}");
        assert!(out.contains("replaced, their hash differed from the manifest: iris"));
        assert_eq!(fs::read(repo.path("mods/iris-1.0.0.jar")).unwrap(), b"iris");
    }

    /// Rewrites the manifest with `iris` dropped and `jei` updated under the
    /// same file name, as a pull from another machine does.
    fn pull_an_update(repo: &Repo) -> MapFetch {
        let mut jei = entry("jei", b"jei2");
        jei.downloads = vec!["https://cdn.invalid/jei2.jar".into()];
        manifest(vec![jei])
            .save(&repo.path(manifest::FILE_NAME))
            .unwrap();
        MapFetch::default().with("https://cdn.invalid/jei2.jar", b"jei2")
    }

    #[test]
    fn jars_it_placed_leave_with_the_manifest_and_others_stay() {
        let repo = Repo::new();
        repo.install(false).0.unwrap();
        fs::write(repo.path("mods/tried.jar"), b"x").unwrap();
        let fetch = pull_an_update(&repo);

        let (result, out) = repo.install_with(&fetch, false);
        result.unwrap();
        assert!(
            out.contains("jars: 0 present, 1 downloaded, 0 failed"),
            "{out}"
        );
        assert!(
            out.contains("removed, no longer in the manifest: mods/iris-1.0.0.jar"),
            "{out}"
        );
        assert!(out.contains("left as is: mods/tried.jar"), "{out}");
        assert!(!repo.path("mods/iris-1.0.0.jar").exists());
        assert_eq!(fs::read(repo.path("mods/jei-1.0.0.jar")).unwrap(), b"jei2");
        assert_eq!(fs::read(repo.path("mods/tried.jar")).unwrap(), b"x");
        let installed = Installed::load(repo.root.path()).unwrap();
        assert_eq!(
            installed.jars.keys().collect::<Vec<_>>(),
            ["mods/jei-1.0.0.jar"]
        );
    }

    #[test]
    fn a_jar_it_placed_then_changed_by_hand_is_the_users() {
        let repo = Repo::new();
        repo.install(false).0.unwrap();
        fs::write(repo.path("mods/iris-1.0.0.jar"), b"patched").unwrap();
        fs::write(repo.path("mods/jei-1.0.0.jar"), b"patched").unwrap();
        let fetch = pull_an_update(&repo);

        let (result, out) = repo.install_with(&fetch, false);
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Blocked);
        assert!(out.contains("not replaced: jei"), "{out}");
        assert!(out.contains("left as is: mods/iris-1.0.0.jar"), "{out}");
        assert_eq!(
            fs::read(repo.path("mods/iris-1.0.0.jar")).unwrap(),
            b"patched"
        );
        assert!(Installed::load(repo.root.path()).unwrap().jars.is_empty());
    }

    #[test]
    fn failures_say_why_and_what_to_do() {
        let repo = Repo::new();
        let fetch = MapFetch::default().with("https://cdn.invalid/jei.jar", b"jei");
        let (result, out) = repo.install_with(&fetch, false);
        let err = result.unwrap_err();
        assert!(err.to_string().contains("1 jar(s) could not be downloaded"));
        assert!(
            out.contains("jars: 0 present, 1 downloaded, 1 failed"),
            "{out}"
        );
        assert!(
            out.contains("failed: iris\n  from https://cdn.invalid/iris.jar: 404 for https://cdn.invalid/iris.jar\n    hint: none"),
            "{out}"
        );
    }

    #[test]
    fn reloads_the_mode_loaded_in_the_instance() {
        let repo = Repo::new();
        let instance = repo.machine.path().join("profiles/Test Pack");
        fs::create_dir_all(&instance).unwrap();
        let mut instances = Instances::default();
        instances.set(PACK, instance.clone());
        instances.save(&repo.instances_file()).unwrap();

        let (result, out) = repo.install(false);
        result.unwrap();
        assert!(!out.contains("mode"), "no mode loaded yet: {out}");

        fs::write(
            repo.path("modes.toml"),
            "[modes.server]\nremove = [\"iris\"]\n",
        )
        .unwrap();
        InstanceState {
            mode: Some("server".into()),
            ..Default::default()
        }
        .save(&instance)
        .unwrap();
        let (result, out) = repo.install_with(&NoFetch, false);
        result.unwrap();
        assert!(out.contains("mode `server` loaded into"), "{out}");
        assert!(out.contains("jars copied: mods/jei-1.0.0.jar\n"), "{out}");
        assert!(instance.join("mods/jei-1.0.0.jar").is_file());
        assert!(!instance.join("mods/iris-1.0.0.jar").exists());
    }

    #[test]
    fn reload_stops_on_drift_and_leaves_the_instance() {
        let repo = Repo::new();
        let instance = repo.machine.path().join("profiles/Test Pack");
        fs::create_dir_all(instance.join("config")).unwrap();
        let mut instances = Instances::default();
        instances.set(PACK, instance.clone());
        instances.save(&repo.instances_file()).unwrap();
        fs::write(repo.path("modes.toml"), "[default]\n").unwrap();
        fs::write(instance.join("config/a.toml"), "changed in game").unwrap();
        let mut state = InstanceState {
            mode: Some("default".into()),
            ..Default::default()
        };
        state.files.insert("config/a.toml".into(), "0".repeat(64));
        state.save(&instance).unwrap();

        let (result, _) = repo.install(false);
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Blocked);
        assert!(!instance.join("mods").exists());
    }

    #[test]
    fn repository_without_manifest_is_a_usage_error() {
        let repo = Repo::new();
        fs::remove_file(repo.path(manifest::FILE_NAME)).unwrap();
        let err = repo.install(false).0.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.hint().contains("modpack manifest"));
    }
}
