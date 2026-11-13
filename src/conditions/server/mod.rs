//! `server-starts`: a dedicated server with the variant's mods reaches `Done`.
//!
//! The server of the pack's loader and Minecraft version is installed once
//! into the machine's cache, with the loader's official installer. Each check
//! then copies it into a temporary directory under the cache (hard links when
//! the filesystem allows), adds the variant's jars but the client-only ones
//! (`env.server = "unsupported"`), the base config then the
//! variant's overrides, accepts the EULA, and starts the server without its
//! interface until the `Done` line of its log.

mod java;
mod loader;
mod run;

use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::de::DeserializeOwned;

use super::Status;
use crate::error::{Error, Result};
use crate::model::manifest::{Manifest, Pack, Support};
use crate::model::variants::Resolved;
use crate::store::{self, download::Fetch};
use loader::Loader;

/// A step of the check: its error is the reason of the failure.
type Step<T> = std::result::Result<T, String>;

/// File of an installed server holding the Java major version it needs. It is
/// written last: a server without it is an unfinished install.
const READY: &str = ".modpack-java";

/// What `server-starts` needs from the machine.
pub struct Machine<'a> {
    pub fetch: &'a dyn Fetch,
    /// The Java that runs installers and servers.
    pub java: PathBuf,
    /// Where installed servers are kept between checks.
    pub cache_dir: PathBuf,
}

impl<'a> Machine<'a> {
    /// `$JAVA_HOME/bin/java`, else `java` from the PATH; servers cached in
    /// `<user cache dir>/modpack/servers`.
    pub fn from_env(fetch: &'a dyn Fetch) -> Result<Self> {
        let java = match std::env::var_os("JAVA_HOME").filter(|v| !v.is_empty()) {
            Some(home) => PathBuf::from(home).join("bin").join("java"),
            None => PathBuf::from("java"),
        };
        let dirs = directories::BaseDirs::new().ok_or_else(|| {
            Error::internal(
                "cannot find the user's home directory",
                "set HOME to the user's home directory",
            )
        })?;
        Ok(Self {
            fetch,
            java,
            cache_dir: dirs.cache_dir().join("modpack").join("servers"),
        })
    }
}

pub fn server_starts(
    root: &Path,
    manifest: &Manifest,
    variant: &Resolved,
    timeout: Duration,
    machine: &Machine<'_>,
) -> Status {
    let pack = &manifest.pack;
    let Some(loader) = Loader::of(&pack.loader) else {
        return Status::Skipped(format!(
            "no scriptable server install for the loader `{}` (handled: {})",
            pack.loader,
            loader::SUPPORTED
        ));
    };
    let have = match java::major_version(&machine.java) {
        Ok(Some(have)) => have,
        Ok(None) => {
            return Status::Skipped(format!(
                "no Java found (`{}`); set JAVA_HOME or put java on the PATH to run this check",
                machine.java.display()
            ));
        }
        Err(reason) => return Status::Failed(reason),
    };
    let start = || -> Step<Status> {
        let server = installed(loader, pack, have, machine)?;
        let run_dir = prepare(root, manifest, variant, &server, machine)?;
        let mut command = Command::new(&machine.java);
        command
            .args(loader.launch_args(pack))
            .current_dir(run_dir.path());
        tracing::info!(variant = %variant.name, dir = %run_dir.path().display(), "starting the server");
        Ok(run::until_done(command, timeout))
    };
    start().unwrap_or_else(Status::Failed)
}

/// The cached server of the pack, installed first if needed.
fn installed(loader: Loader, pack: &Pack, java: u32, machine: &Machine<'_>) -> Step<PathBuf> {
    let dir = machine.cache_dir.join(safe_name(&format!(
        "{}-{}-{}",
        loader.name(),
        pack.minecraft,
        pack.loader_version
    )));
    let ready = fs::read_to_string(dir.join(READY))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok());
    if let Some(required) = ready {
        java::check(java, required, &pack.minecraft, &machine.java)?;
        return Ok(dir);
    }

    let required = java::required(&pack.minecraft, machine.fetch)?;
    java::check(java, required, &pack.minecraft, &machine.java)?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(io_err("removing", &dir))?;
    }
    fs::create_dir_all(&machine.cache_dir).map_err(io_err("creating", &machine.cache_dir))?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&machine.cache_dir)
        .map_err(io_err("creating a directory in", &machine.cache_dir))?;
    loader.install(pack, staging.path(), &machine.java, machine.fetch)?;
    fs::write(staging.path().join(READY), required.to_string())
        .map_err(io_err("writing in", staging.path()))?;
    let staging = staging.keep();
    if let Err(e) = fs::rename(&staging, &dir) {
        let _ = fs::remove_dir_all(&staging);
        // Another check may have installed the same server meanwhile.
        if !dir.join(READY).is_file() {
            return Err(format!("cannot write {}: {e}", dir.display()));
        }
    }
    Ok(dir)
}

/// A temporary server directory for the variant, removed when dropped.
fn prepare(
    root: &Path,
    manifest: &Manifest,
    variant: &Resolved,
    server: &Path,
    machine: &Machine<'_>,
) -> Step<tempfile::TempDir> {
    let runs = machine.cache_dir.join(".runs");
    fs::create_dir_all(&runs).map_err(io_err("creating", &runs))?;
    let run = tempfile::Builder::new()
        .prefix(&format!("{}-", safe_name(&variant.name)))
        .tempdir_in(&runs)
        .map_err(io_err("creating a directory in", &runs))?;
    let dir = run.path();
    copy_tree(server, dir, true)?;

    let mods = dir.join(store::MODS_DIR);
    fs::create_dir_all(&mods).map_err(io_err("creating", &mods))?;
    let mut missing = Vec::new();
    let mut local_missing = Vec::new();
    // A client-only mod is left out, as a server installed from the .mrpack
    // leaves it out.
    let on_server = variant
        .keys
        .iter()
        .filter_map(|k| manifest.get(k))
        .filter(|e| e.env.server != Support::Unsupported);
    for entry in on_server {
        let jar = store::jar_path(root, entry).map_err(reason)?;
        if !jar.is_file() {
            match entry.is_local() {
                true => local_missing.push(entry.path.as_str()),
                false => missing.push(entry.key.as_str()),
            }
            continue;
        }
        let name = jar.file_name().expect("jar_path ends with a file name");
        link_or_copy(&jar, &mods.join(name))?;
    }
    if !missing.is_empty() {
        return Err(format!(
            "jars missing from mods/: {}; run `modpack install`",
            missing.join(", ")
        ));
    }
    // Git holds the jars of mods-local/: `modpack install` cannot restore
    // them.
    if !local_missing.is_empty() {
        return Err(format!(
            "local jars missing from mods-local/: {}; restore them from git with `git checkout -- {}`",
            local_missing.join(", "),
            local_missing.join(" ")
        ));
    }

    for config in [
        root.join("config"),
        root.join("overrides").join(&variant.name).join("config"),
    ] {
        if config.is_dir() {
            copy_tree(&config, &dir.join("config"), false)?;
        }
    }
    write(&dir.join("eula.txt"), "eula=true\n")?;
    // A free port, so that a server already running on this machine does not
    // make the check fail.
    let port = TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map_err(|e| format!("cannot find a free port: {e}"))?
        .port();
    write(
        &dir.join("server.properties"),
        &format!("server-port={port}\n"),
    )?;
    Ok(run)
}

/// Copies `from` into `to`, replacing files; with `link`, hard-links files
/// when the filesystem allows. The server only reads what is linked.
fn copy_tree(from: &Path, to: &Path, link: bool) -> Step<()> {
    fs::create_dir_all(to).map_err(io_err("creating", to))?;
    for entry in fs::read_dir(from).map_err(io_err("reading", from))? {
        let entry = entry.map_err(io_err("reading", from))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if entry
            .file_type()
            .map_err(io_err("reading", &source))?
            .is_dir()
        {
            copy_tree(&source, &target, link)?;
        } else if link {
            link_or_copy(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(io_err("copying", &source))?;
        }
    }
    Ok(())
}

fn link_or_copy(from: &Path, to: &Path) -> Step<()> {
    if fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)
        .map(|_| ())
        .map_err(io_err("copying", from))
}

fn write(path: &Path, text: &str) -> Step<()> {
    fs::write(path, text).map_err(io_err("writing", path))
}

/// Downloads `url` into the file `to`.
fn download(fetch: &dyn Fetch, url: &str, to: &Path) -> Step<()> {
    let mut body = fetch.get(url).map_err(reason)?;
    let mut file = fs::File::create(to).map_err(io_err("creating", to))?;
    io::copy(&mut body, &mut file).map_err(|e| format!("cannot download {url}: {e}"))?;
    Ok(())
}

fn get_json<T: DeserializeOwned>(fetch: &dyn Fetch, url: &str) -> Step<T> {
    serde_json::from_reader(fetch.get(url).map_err(reason)?)
        .map_err(|e| format!("unexpected answer from {url}: {e}"))
}

/// An error of the CLI as the reason of a failure.
fn reason(err: Error) -> String {
    let text = err.to_string();
    text.strip_prefix("error: ").unwrap_or(&text).to_owned()
}

fn io_err(doing: &'static str, path: &Path) -> impl FnOnce(io::Error) -> String {
    let path = path.display().to_string();
    move |e| format!("{doing} {path}: {e}")
}

/// A name usable as a directory name on every system.
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
pub(super) mod tests;
