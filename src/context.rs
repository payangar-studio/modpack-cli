//! Context shared by every command handler.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::prompt::Prompt;

/// Name of the directory that marks the root of a modpack repository.
pub const SOCLE_DIR: &str = ".modpack";

#[derive(Debug, Clone)]
pub struct Context {
    /// Root of the modpack repository, if the working directory is inside one.
    pub root: Option<PathBuf>,
    pub prompt: Prompt,
    /// `-v` count minus `-q` count.
    pub verbosity: i8,
}

impl Context {
    /// Builds the context from the current directory and the process environment.
    pub fn detect(verbosity: i8) -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let ctx = Self {
            root: find_root(&cwd),
            prompt: Prompt::new(crate::prompt::detect_interactive()),
            verbosity,
        };
        tracing::debug!(root = ?ctx.root, interactive = ctx.prompt.is_interactive(), "context");
        Ok(ctx)
    }

    /// The repository root, for commands that only work inside a modpack repository.
    pub fn require_root(&self) -> Result<&Path> {
        self.root.as_deref().ok_or_else(|| {
            Error::usage(
                format!("not inside a modpack repository (no {SOCLE_DIR}/ found in this directory or above)"),
                "run the command from a modpack repository, or create one with `modpack init`",
            )
        })
    }
}

/// Walks up from `start` to the first directory holding `.modpack/`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(SOCLE_DIR).is_dir())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn root_is_found_from_a_nested_directory() {
        let repo = fixtures().join("repo");
        assert_eq!(find_root(&repo.join("config/nested")), Some(repo.clone()));
        assert_eq!(find_root(&repo), Some(repo));
    }

    #[test]
    fn no_root_outside_a_repository() {
        assert_eq!(find_root(&fixtures()), None);
        let ctx = Context {
            root: None,
            prompt: Prompt::new(false),
            verbosity: 0,
        };
        let err = ctx.require_root().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.hint().contains("modpack init"));
    }
}
