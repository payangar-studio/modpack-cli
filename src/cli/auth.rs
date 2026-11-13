//! `modpack auth`: stores the platform secrets in the system keyring.
//!
//! Values are typed with masked input in the user's own terminal, never
//! passed as arguments, so they stay out of shell history and AI conversations.
//! This is why this command does not go through [`crate::prompt`]: without a
//! terminal there is no argument to fall back on, only the environment.

use dialoguer::Password;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::secrets::{Secret, SecretName, Secrets, Source};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Secret to store; every secret when omitted
    #[arg(value_enum, conflicts_with_all = ["status", "remove"])]
    pub secret: Option<SecretName>,
    /// Tell, for each secret, whether it is found and where, without its value
    #[arg(long, conflicts_with = "remove")]
    pub status: bool,
    /// Remove a secret from the system keyring
    #[arg(long, value_enum, value_name = "SECRET")]
    pub remove: Option<SecretName>,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    if args.status {
        print!("{}", status(&Secrets::system()));
        return Ok(());
    }
    if let Some(name) = args.remove {
        println!("{}", remove(&Secrets::system(), name)?);
        return Ok(());
    }
    let names = match args.secret {
        Some(name) => vec![name],
        None => SecretName::ALL.to_vec(),
    };
    if !ctx.prompt.is_interactive() {
        return Err(no_terminal(&names));
    }
    let secrets = Secrets::system();
    secrets.require_keyring(names[0])?;
    for name in names {
        let stored = secrets.in_keyring(name)?;
        let skip = if stored {
            "empty to keep the stored one"
        } else {
            "empty to skip"
        };
        let value = Password::new()
            .with_prompt(format!("{} ({skip})", name.label()))
            .allow_empty_password(true)
            .interact()?;
        println!("{}", save(&secrets, name, value)?);
    }
    Ok(())
}

fn no_terminal(names: &[SecretName]) -> Error {
    let vars: Vec<_> = names.iter().map(|name| name.env_var()).collect();
    Error::usage(
        "`modpack auth` reads secrets with masked input in a terminal, and there is none",
        format!(
            "run it from your own terminal, or set {} in the environment instead",
            vars.join(", ")
        ),
    )
}

/// Stores a typed value; an empty one stores nothing.
fn save(secrets: &Secrets, name: SecretName, value: String) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(format!("{name}: unchanged"));
    }
    secrets.store(name, &Secret::new(value))?;
    let mut message = format!("{name}: stored in the system keyring");
    if secrets.locate(name) == Ok(Some(Source::Environment)) {
        message.push_str(&format!(
            "\n  note: {} is set and takes precedence over the keyring",
            name.env_var()
        ));
    }
    Ok(message)
}

fn remove(secrets: &Secrets, name: SecretName) -> Result<String> {
    Ok(if secrets.remove(name)? {
        format!("{name}: removed from the system keyring")
    } else {
        format!("{name}: not in the system keyring, nothing to remove")
    })
}

/// One line per secret: where it is found, never its value.
fn status(secrets: &Secrets) -> String {
    let mut out = String::new();
    if let Some(why) = secrets.keyring_unavailable() {
        out.push_str(&format!("system keyring: unavailable ({why})\n"));
    }
    for name in SecretName::ALL {
        let place = match secrets.locate(name) {
            Ok(Some(Source::Environment)) => {
                format!("found in the environment ({})", name.env_var())
            }
            Ok(Some(Source::Keyring)) => "found in the system keyring".to_string(),
            Ok(None) => "not found".to_string(),
            Err(why) => format!("unknown, the system keyring failed: {why}"),
        };
        out.push_str(&format!("{name}: {place}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use keyring_core::{CredentialStore, mock};

    use super::*;
    use crate::error::ErrorKind;

    const VALUE: &str = "s3cr3t-value-never-shown";

    fn secrets(env: &[(&str, &str)], keyring: bool) -> Secrets {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let store = if keyring {
            let store: Arc<CredentialStore> = mock::Store::new().unwrap();
            Ok(store)
        } else {
            Err("no Secret Service".to_string())
        };
        Secrets::new(move |var| env.get(var).cloned(), store)
    }

    #[test]
    fn without_terminal_auth_points_to_the_environment() {
        let err = no_terminal(&[SecretName::ModrinthToken]);
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(
            err.hint()
                .contains("set MODPACK_MODRINTH_TOKEN in the environment")
        );
    }

    #[test]
    fn status_tells_where_each_secret_is_without_its_value() {
        let secrets = secrets(&[("MODPACK_MODRINTH_TOKEN", VALUE)], true);
        secrets
            .store(SecretName::CurseforgeApiKey, &Secret::new(VALUE))
            .unwrap();
        assert_eq!(
            status(&secrets),
            "modrinth-token: found in the environment (MODPACK_MODRINTH_TOKEN)\n\
             curseforge-upload-token: not found\n\
             curseforge-api-key: found in the system keyring\n"
        );
    }

    #[test]
    fn status_says_when_the_keyring_is_unavailable() {
        let report = status(&secrets(&[], false));
        assert!(report.starts_with("system keyring: unavailable (no Secret Service)\n"));
        assert!(report.contains("curseforge-api-key: not found\n"));
    }

    #[test]
    fn save_stores_a_trimmed_value_and_skips_an_empty_one() {
        let secrets = secrets(&[], true);
        let name = SecretName::CurseforgeUploadToken;
        assert_eq!(
            save(&secrets, name, "  \n".into()).unwrap(),
            "curseforge-upload-token: unchanged"
        );
        assert!(!secrets.in_keyring(name).unwrap());
        let message = save(&secrets, name, format!(" {VALUE}\n")).unwrap();
        assert!(!message.contains(VALUE));
        assert_eq!(secrets.get(name).unwrap().expose(), VALUE);
    }

    #[test]
    fn save_warns_when_the_environment_overrides_the_keyring() {
        let secrets = secrets(&[("MODPACK_MODRINTH_TOKEN", "other")], true);
        let message = save(&secrets, SecretName::ModrinthToken, VALUE.into()).unwrap();
        assert!(message.contains("MODPACK_MODRINTH_TOKEN is set and takes precedence"));
    }

    #[test]
    fn remove_reports_whether_the_secret_was_there() {
        let secrets = secrets(&[], true);
        let name = SecretName::ModrinthToken;
        secrets.store(name, &Secret::new(VALUE)).unwrap();
        assert_eq!(
            remove(&secrets, name).unwrap(),
            "modrinth-token: removed from the system keyring"
        );
        assert_eq!(
            remove(&secrets, name).unwrap(),
            "modrinth-token: not in the system keyring, nothing to remove"
        );
    }
}
