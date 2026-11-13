//! Installing and launching the dedicated server of each loader `server-starts`
//! handles: those whose server install runs without interaction.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use super::{Step, download, get_json, run};
use crate::model::manifest::Pack;
use crate::store::download::Fetch;

/// Fabric's list of installer versions, newest first.
pub const FABRIC_INSTALLERS: &str = "https://meta.fabricmc.net/v2/versions/installer";
/// Maven repository of NeoForge's installers.
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

/// Loaders `server-starts` can install.
pub const SUPPORTED: &str = "fabric, neoforge";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    Fabric,
    NeoForge,
}

impl Loader {
    /// The loader of the pack, by the name the manifest gives it.
    pub fn of(name: &str) -> Option<Self> {
        match name {
            "fabric" => Some(Loader::Fabric),
            "neoforge" => Some(Loader::NeoForge),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Loader::Fabric => "fabric",
            Loader::NeoForge => "neoforge",
        }
    }

    /// Installs the server of `pack` into the empty directory `dir`, with the
    /// loader's official installer.
    pub fn install(self, pack: &Pack, dir: &Path, java: &Path, fetch: &dyn Fetch) -> Step<()> {
        // The installer goes next to the server, not in it.
        let installer = dir.with_extension("installer.jar");
        let result = match self {
            Loader::Fabric => {
                #[derive(Deserialize)]
                struct Installer {
                    url: String,
                    stable: bool,
                }
                let installers: Vec<Installer> = get_json(fetch, FABRIC_INSTALLERS)?;
                let url = installers
                    .into_iter()
                    .find(|i| i.stable)
                    .map(|i| i.url)
                    .ok_or_else(|| format!("no stable Fabric installer in {FABRIC_INSTALLERS}"))?;
                download(fetch, &url, &installer)?;
                run_installer(
                    self,
                    java,
                    &installer,
                    &[
                        "server".as_ref(),
                        "-dir".as_ref(),
                        dir.as_os_str(),
                        "-mcversion".as_ref(),
                        pack.minecraft.as_ref(),
                        "-loader".as_ref(),
                        pack.loader_version.as_ref(),
                        "-downloadMinecraft".as_ref(),
                    ],
                    dir,
                )
            }
            Loader::NeoForge => {
                let v = &pack.loader_version;
                download(
                    fetch,
                    &format!("{NEOFORGE_MAVEN}/{v}/neoforge-{v}-installer.jar"),
                    &installer,
                )?;
                run_installer(
                    self,
                    java,
                    &installer,
                    &["--install-server".as_ref(), dir.as_os_str()],
                    dir,
                )
            }
        };
        let _ = std::fs::remove_file(&installer);
        result
    }

    /// Java arguments that start the installed server without its interface,
    /// from the server directory.
    pub fn launch_args(self, pack: &Pack) -> Vec<String> {
        match self {
            Loader::Fabric => vec![
                "-jar".into(),
                "fabric-server-launch.jar".into(),
                "nogui".into(),
            ],
            Loader::NeoForge => {
                let file = if cfg!(windows) {
                    "win_args.txt"
                } else {
                    "unix_args.txt"
                };
                vec![
                    format!(
                        "@libraries/net/neoforged/neoforge/{}/{file}",
                        pack.loader_version
                    ),
                    "nogui".into(),
                ]
            }
        }
    }
}

fn run_installer(
    loader: Loader,
    java: &Path,
    installer: &Path,
    args: &[&std::ffi::OsStr],
    dir: &Path,
) -> Step<()> {
    tracing::info!(loader = loader.name(), dir = %dir.display(), "installing the server");
    let output = Command::new(java)
        .arg("-jar")
        .arg(installer)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run Java (`{}`): {e}", java.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Err(format!(
        "the {} server installer failed ({}); last lines of its output:\n{}",
        loader.name(),
        output.status,
        run::last_lines(&text)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(loader: &str, version: &str) -> Pack {
        Pack {
            minecraft: "1.21.1".into(),
            loader: loader.into(),
            loader_version: version.into(),
        }
    }

    #[test]
    fn only_fabric_and_neoforge_are_handled() {
        assert_eq!(Loader::of("fabric"), Some(Loader::Fabric));
        assert_eq!(Loader::of("neoforge"), Some(Loader::NeoForge));
        assert_eq!(Loader::of("forge"), None);
        assert_eq!(Loader::of("quilt"), None);
    }

    #[test]
    fn launch_args_follow_each_loader() {
        assert_eq!(
            Loader::Fabric.launch_args(&pack("fabric", "0.16.9")),
            ["-jar", "fabric-server-launch.jar", "nogui"]
        );
        let args = Loader::NeoForge.launch_args(&pack("neoforge", "21.1.77"));
        assert!(args[0].starts_with("@libraries/net/neoforged/neoforge/21.1.77/"));
        assert!(args[0].ends_with("_args.txt"));
        assert_eq!(args[1], "nogui");
    }
}
