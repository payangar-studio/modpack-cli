//! `modpack mode list`: lists the modes and shows the loaded one.

use std::io::Write;
use std::path::Path;

use crate::context::Context;
use crate::error::Result;
use crate::mode;
use crate::model::instance::Instances;
use crate::model::modes::DEFAULT_MODE;

#[derive(Debug, clap::Args)]
pub struct Args {}

pub fn run(_args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    list(
        root,
        &Instances::default_path()?,
        &mut std::io::stdout().lock(),
    )
}

/// One line per mode, the default one first: `*` marks the mode loaded in
/// the pack's instance on this machine.
fn list(root: &Path, instances_file: &Path, out: &mut dyn Write) -> Result<()> {
    let modes = mode::load_modes(root)?;
    let loaded = mode::loaded_mode(root, instances_file)?;
    let width = modes.names().iter().map(|n| n.len()).max().unwrap_or(0);
    for name in modes.names() {
        let description = if name == DEFAULT_MODE {
            modes.default.description.as_deref()
        } else {
            modes.modes[name].description.as_deref()
        };
        let mark = if loaded.as_deref() == Some(name) {
            '*'
        } else {
            ' '
        };
        let line = format!("{mark} {name:width$}  {}", description.unwrap_or_default());
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::instance::InstanceState;
    use crate::model::socle::{InitValues, SocleConfig};
    use std::fs;

    fn run_list(root: &Path, instances_file: &Path) -> String {
        let mut out = Vec::new();
        list(root, instances_file, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn lists_the_modes_and_marks_the_loaded_one() {
        let root = tempfile::tempdir().unwrap();
        let machine = tempfile::tempdir().unwrap();
        SocleConfig {
            version: "0.1.0".into(),
            init: InitValues {
                name: "P".into(),
                minecraft: "1.21.1".into(),
                loader: "neoforge".into(),
                loader_version: "21.1.77".into(),
                author: "a".into(),
            },
        }
        .save(root.path())
        .unwrap();
        fs::write(
            root.path().join("modes.toml"),
            "[default]\ndescription = \"Full pack\"\n[modes.server]\ndescription = \"Dedicated server\"\n[modes.lowend]\n",
        )
        .unwrap();
        let instances_file = machine.path().join("instances.toml");

        assert_eq!(
            run_list(root.path(), &instances_file),
            "  default  Full pack\n  lowend\n  server   Dedicated server\n"
        );

        let instance = machine.path().join("profiles/P");
        fs::create_dir_all(&instance).unwrap();
        let mut instances = Instances::default();
        instances.set("P", instance.clone());
        instances.save(&instances_file).unwrap();
        InstanceState {
            mode: Some("server".into()),
            ..Default::default()
        }
        .save(&instance)
        .unwrap();
        assert!(run_list(root.path(), &instances_file).contains("* server   Dedicated server\n"));
    }
}
