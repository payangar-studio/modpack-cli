//! Error model shared by every command.
//!
//! Every error tells the user what happened and what to do about it, and maps
//! to one of a few exit codes so scripts, git hooks and CI can react to it.

use std::fmt;
use std::process::ExitCode;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Exit code of a successful run.
pub const EXIT_SUCCESS: u8 = 0;
/// Exit code of an unexpected failure, or of a command not implemented yet.
pub const EXIT_INTERNAL: u8 = 1;
/// Exit code of a usage error, such as a missing argument. Matches clap's own.
pub const EXIT_USAGE: u8 = 2;
/// Exit code of a refusal that `--force` can override (drift, file edited by hand).
pub const EXIT_BLOCKED: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Usage,
    Blocked,
    Internal,
}

impl ErrorKind {
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorKind::Usage => EXIT_USAGE,
            ErrorKind::Blocked => EXIT_BLOCKED,
            ErrorKind::Internal => EXIT_INTERNAL,
        }
    }
}

type Source = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Hint given when an error carries no specific advice.
const DEFAULT_HINT: &str = "check the cause above; if it looks like a bug, report it at https://github.com/payangar-studio/modpack-cli/issues";

/// An error as shown to the user: what happened, what to do, and the
/// underlying cause when there is one.
///
/// It deliberately does not implement `std::error::Error`, so that `?` turns
/// any standard error into an internal one through the blanket `From` below.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    what: String,
    hint: String,
    source: Option<Source>,
}

impl Error {
    fn new(kind: ErrorKind, what: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            kind,
            what: what.into(),
            hint: hint.into(),
            source: None,
        }
    }

    /// The command line is wrong or incomplete.
    pub fn usage(what: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, what, hint)
    }

    /// The command refuses to go on; `hint` should mention `--force` when it applies.
    pub fn blocked(what: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(ErrorKind::Blocked, what, hint)
    }

    /// Something failed that the user did not cause.
    pub fn internal(what: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, what, hint)
    }

    /// A value is missing and there is no terminal to ask for it.
    pub fn missing_value(arg: &str) -> Self {
        Self::usage(
            format!("missing value for {arg}, and no terminal to ask for it"),
            format!("pass {arg}"),
        )
    }

    /// The command exists in the CLI but has no implementation yet.
    pub fn not_implemented(command: &str) -> Self {
        Self::internal(
            format!("`modpack {command}` is not implemented yet"),
            "use a later release of modpack-cli",
        )
    }

    /// Attaches the underlying cause.
    pub fn with_source(mut self, source: impl Into<Source>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn hint(&self) -> &str {
        &self.hint
    }

    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(self.kind.exit_code())
    }
}

impl<E> From<E> for Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(source: E) -> Self {
        Self::internal(source.to_string(), DEFAULT_HINT).with_source(source)
    }
}

/// Renders the error for stderr: `what`, the chain of causes, then the hint.
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error: {}", self.what)?;
        let mut cause: Option<&(dyn std::error::Error + 'static)> = match &self.source {
            Some(source) => Some(source.as_ref()),
            None => None,
        };
        let mut previous = self.what.clone();
        while let Some(err) = cause {
            let text = err.to_string();
            // An error converted with `?` already uses its source as `what`.
            if text != previous {
                write!(f, "\n  caused by: {text}")?;
            }
            previous = text;
            cause = err.source();
        }
        write!(f, "\n  hint: {}", self.hint)
    }
}

/// Adds a description of what was being done to a failing result.
pub trait Context<T> {
    /// Turns the error into an internal one whose message is `what`.
    fn context(self, what: impl Into<String>) -> Result<T>;
}

impl<T, E> Context<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn context(self, what: impl Into<String>) -> Result<T> {
        self.map_err(|e| Error::internal(what, DEFAULT_HINT).with_source(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_distinct() {
        let codes = [
            EXIT_SUCCESS,
            ErrorKind::Internal.exit_code(),
            ErrorKind::Usage.exit_code(),
            ErrorKind::Blocked.exit_code(),
        ];
        for (i, a) in codes.iter().enumerate() {
            for b in &codes[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn display_says_what_happened_and_what_to_do() {
        let err = Error::blocked("mods/ drifted", "rerun with --force");
        assert_eq!(
            err.to_string(),
            "error: mods/ drifted\n  hint: rerun with --force"
        );
    }

    #[test]
    fn question_mark_gives_an_internal_error() {
        fn fails() -> Result<()> {
            Err(std::io::Error::other("disk on fire"))?;
            Ok(())
        }
        let err = fails().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert_eq!(
            err.to_string(),
            format!("error: disk on fire\n  hint: {DEFAULT_HINT}")
        );
    }

    #[test]
    fn context_keeps_the_cause() {
        let res: std::result::Result<(), _> = Err(std::io::Error::other("denied"));
        let err = res.context("reading mods.lock.toml").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert!(
            err.to_string()
                .starts_with("error: reading mods.lock.toml\n  caused by: denied\n")
        );
    }

    #[test]
    fn missing_value_names_the_argument() {
        let err = Error::missing_value("--mc-version");
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("--mc-version"));
        assert_eq!(err.hint(), "pass --mc-version");
    }
}
