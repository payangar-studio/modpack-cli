//! "Ask, or read the argument": the single way a command obtains a value.
//!
//! A value passed as an argument is used as is, without any question. A
//! missing value is asked for when a terminal is attached, and is an error
//! naming the argument to pass otherwise (AI agent, git hook, CI).

use std::fmt::Display;
use std::io::IsTerminal;

use dialoguer::{Confirm, Input, Password, Select};

use crate::error::{Error, Result};

/// Whether this process can ask questions: stdin and stdout are terminals and
/// the `CI` variable is not set.
pub fn detect_interactive() -> bool {
    let ci = std::env::var("CI").is_ok_and(|v| !v.is_empty() && v != "false" && v != "0");
    !ci && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

#[derive(Debug, Clone, Copy)]
pub struct Prompt {
    interactive: bool,
}

impl Prompt {
    pub fn new(interactive: bool) -> Self {
        Self { interactive }
    }

    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    /// Free text. `arg` is the argument that carries the value, e.g. `--name`.
    pub fn text(&self, value: Option<String>, arg: &str, question: &str) -> Result<String> {
        self.resolve(value, arg, || {
            Ok(Input::<String>::new()
                .with_prompt(question)
                .interact_text()?)
        })
    }

    /// One item out of `items`. A value passed as an argument is not checked
    /// against `items`: validating it is the caller's job.
    pub fn select<T: Display + Clone>(
        &self,
        value: Option<T>,
        arg: &str,
        question: &str,
        items: &[T],
    ) -> Result<T> {
        self.resolve(value, arg, || {
            let index = Select::new()
                .with_prompt(question)
                .items(items)
                .default(0)
                .interact()?;
            Ok(items[index].clone())
        })
    }

    /// Yes or no.
    pub fn confirm(
        &self,
        value: Option<bool>,
        arg: &str,
        question: &str,
        default: bool,
    ) -> Result<bool> {
        self.resolve(value, arg, || {
            Ok(Confirm::new()
                .with_prompt(question)
                .default(default)
                .interact()?)
        })
    }

    /// Masked input, for secrets. The value is never echoed.
    pub fn secret(&self, value: Option<String>, arg: &str, question: &str) -> Result<String> {
        self.resolve(value, arg, || {
            Ok(Password::new().with_prompt(question).interact()?)
        })
    }

    fn resolve<T>(
        &self,
        value: Option<T>,
        arg: &str,
        ask: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        match value {
            Some(value) => Ok(value),
            None if self.interactive => ask(),
            None => Err(Error::missing_value(arg)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn never_asked<T>() -> Result<T> {
        panic!("the user was asked although the value was given")
    }

    #[test]
    fn given_value_is_never_asked_for() {
        for interactive in [true, false] {
            let prompt = Prompt::new(interactive);
            let value = prompt
                .resolve(Some("x".to_string()), "--name", never_asked)
                .unwrap();
            assert_eq!(value, "x");
        }
    }

    #[test]
    fn given_values_pass_through_every_variant() {
        // Interactive on purpose: asking would fail or hang without a terminal.
        let prompt = Prompt::new(true);
        assert_eq!(prompt.text(Some("a".into()), "--a", "?").unwrap(), "a");
        assert_eq!(
            prompt.select(Some("b"), "--b", "?", &["a", "b"]).unwrap(),
            "b"
        );
        assert!(!prompt.confirm(Some(false), "--c", "?", true).unwrap());
        assert_eq!(prompt.secret(Some("d".into()), "--d", "?").unwrap(), "d");
    }

    #[test]
    fn missing_value_without_terminal_names_the_argument() {
        let prompt = Prompt::new(false);
        let errors = [
            prompt.text(None, "--name", "Name?").unwrap_err(),
            prompt
                .select(None::<&str>, "--name", "Name?", &["a"])
                .unwrap_err(),
            prompt.confirm(None, "--name", "Name?", true).unwrap_err(),
            prompt.secret(None, "--name", "Name?").unwrap_err(),
        ];
        for err in errors {
            assert_eq!(err.kind(), ErrorKind::Usage);
            assert_eq!(err.hint(), "pass --name");
        }
    }

    #[test]
    fn missing_value_with_terminal_asks() {
        let prompt = Prompt::new(true);
        let value = prompt.resolve(None, "--name", || Ok("asked")).unwrap();
        assert_eq!(value, "asked");
    }
}
