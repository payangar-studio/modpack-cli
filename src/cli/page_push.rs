//! `modpack page push`: pushes the versioned page (`page/`) to the platforms
//! that manage a project by their API, and names the skill to run for the
//! others. Project management happens in a session, never in CI.

use std::io::Write;
use std::path::Path;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::platform::{Capability, Operation, Registry, unsupported};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Push to this platform only
    #[arg(long)]
    pub platform: Option<String>,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    refuse_in_ci(std::env::var("CI").ok().as_deref())?;
    let root = ctx.require_root()?;
    push(
        &Registry::builtin(),
        args.platform.as_deref(),
        root,
        &mut std::io::stdout(),
    )
}

fn refuse_in_ci(ci: Option<&str>) -> Result<()> {
    if ci.is_some_and(|v| !v.is_empty() && v != "false" && v != "0") {
        return Err(Error::usage(
            "`modpack page push` manages the project page, which is never done in CI",
            "run it in a session, from your own machine",
        ));
    }
    Ok(())
}

/// Pushes the page to `only`, or to every platform. A platform that pushes
/// by its API prints its own report.
fn push(registry: &Registry, only: Option<&str>, root: &Path, out: &mut impl Write) -> Result<()> {
    let platforms = match only {
        Some(name) => vec![registry.get(name)?],
        None => registry.all().collect(),
    };
    for platform in platforms {
        let name = platform.name();
        match platform.capability(Operation::PushPage) {
            Capability::Api => platform.push_page(root)?,
            Capability::Skill(skill) => writeln!(
                out,
                "{name}: no API for the page; run the `{skill}` skill to push it"
            )?,
            Capability::Unsupported if only.is_some() => {
                return Err(unsupported(name, Operation::PushPage));
            }
            Capability::Unsupported => writeln!(out, "{name}: no project page, skipped")?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::error::ErrorKind;
    use crate::platform::Platform;
    use crate::platform::fake::FakePlatform;

    /// A platform whose page capability is given, recording the pushes.
    struct Recorder {
        name: &'static str,
        capability: Capability,
        pushed: Arc<Mutex<Vec<String>>>,
    }

    impl Platform for Recorder {
        fn name(&self) -> &str {
            self.name
        }
        fn capability(&self, _: Operation) -> Capability {
            self.capability.clone()
        }
        fn push_page(&self, root: &Path) -> Result<()> {
            self.pushed
                .lock()
                .unwrap()
                .push(format!("{} {}", self.name, root.display()));
            Ok(())
        }
    }

    fn registry(pushed: &Arc<Mutex<Vec<String>>>) -> Registry {
        Registry::new(vec![
            Box::new(Recorder {
                name: "api",
                capability: Capability::Api,
                pushed: pushed.clone(),
            }),
            Box::new(FakePlatform::new("fake")),
            Box::new(Recorder {
                name: "none",
                capability: Capability::Unsupported,
                pushed: pushed.clone(),
            }),
        ])
    }

    fn run_push(registry: &Registry, only: Option<&str>) -> (Result<()>, String) {
        let mut out = Vec::new();
        let result = push(registry, only, Path::new("/repo"), &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    #[test]
    fn every_platform_gets_the_page_its_own_way() {
        let pushed = Arc::default();
        let (result, out) = run_push(&registry(&pushed), None);
        result.unwrap();
        assert_eq!(*pushed.lock().unwrap(), ["api /repo"]);
        assert_eq!(
            out,
            "fake: no API for the page; run the `modpack-fake-page` skill to push it\n\
             none: no project page, skipped\n"
        );
    }

    #[test]
    fn platform_option_pushes_to_that_platform_only() {
        let pushed = Arc::default();
        let registry = registry(&pushed);

        let (result, out) = run_push(&registry, Some("fake"));
        result.unwrap();
        assert!(pushed.lock().unwrap().is_empty());
        assert!(out.contains("`modpack-fake-page`"));

        let (result, out) = run_push(&registry, Some("api"));
        result.unwrap();
        assert_eq!(*pushed.lock().unwrap(), ["api /repo"]);
        assert!(out.is_empty());
    }

    #[test]
    fn named_platform_must_exist_and_support_the_page() {
        let pushed = Arc::default();
        let registry = registry(&pushed);
        let (result, _) = run_push(&registry, Some("nope"));
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Usage);
        let (result, _) = run_push(&registry, Some("none"));
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not support `push_page`"));
    }

    #[test]
    fn refused_in_ci() {
        assert_eq!(
            refuse_in_ci(Some("true")).unwrap_err().kind(),
            ErrorKind::Usage
        );
        assert!(refuse_in_ci(Some("false")).is_ok());
        assert!(refuse_in_ci(Some("")).is_ok());
        assert!(refuse_in_ci(None).is_ok());
    }
}
