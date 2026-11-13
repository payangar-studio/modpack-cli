//! Platform secrets, read from the environment first, then from the system
//! keyring.
//!
//! A value only ever lives in a [`Secret`], which hides it from `Debug` and
//! `Display`. Errors and logs built here name the secret, never its value, and
//! never carry a keyring error as is: some of them hold the raw value.

use std::fmt;
use std::sync::Arc;

use keyring_core::{CredentialStore, Error as KeyringError};

use crate::error::{Error, Result};

/// Keyring service every secret is stored under; the account is the secret's name.
pub const KEYRING_SERVICE: &str = "modpack-cli";

/// The secrets the CLI knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SecretName {
    /// Modrinth personal access token
    ModrinthToken,
    /// CurseForge upload token
    CurseforgeUploadToken,
    /// CurseForge Core API key
    CurseforgeApiKey,
}

impl SecretName {
    pub const ALL: [SecretName; 3] = [
        SecretName::ModrinthToken,
        SecretName::CurseforgeUploadToken,
        SecretName::CurseforgeApiKey,
    ];

    /// The name typed on the command line, also the keyring account.
    pub fn as_str(self) -> &'static str {
        match self {
            SecretName::ModrinthToken => "modrinth-token",
            SecretName::CurseforgeUploadToken => "curseforge-upload-token",
            SecretName::CurseforgeApiKey => "curseforge-api-key",
        }
    }

    /// The environment variable that overrides the keyring.
    pub fn env_var(self) -> &'static str {
        match self {
            SecretName::ModrinthToken => "MODPACK_MODRINTH_TOKEN",
            SecretName::CurseforgeUploadToken => "MODPACK_CURSEFORGE_UPLOAD_TOKEN",
            SecretName::CurseforgeApiKey => "MODPACK_CURSEFORGE_API_KEY",
        }
    }

    /// What the secret is, for prompts.
    pub fn label(self) -> &'static str {
        match self {
            SecretName::ModrinthToken => "Modrinth personal access token",
            SecretName::CurseforgeUploadToken => "CurseForge upload token",
            SecretName::CurseforgeApiKey => "CurseForge Core API key",
        }
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A secret value. `Debug` and `Display` never show it; [`Secret::expose`]
/// is the only way to read it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that sends it (an HTTP header, a keyring).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where a secret was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Environment,
    Keyring,
}

/// Reads an environment variable.
type EnvReader = Box<dyn Fn(&str) -> Option<String>>;

/// Access to the secrets: the environment, then the keyring when there is one.
pub struct Secrets {
    env: EnvReader,
    /// The keyring, or why there is none.
    keyring: std::result::Result<Arc<CredentialStore>, String>,
}

impl Secrets {
    /// The process environment and the system keyring. A keyring that cannot
    /// be reached (no Secret Service, as in CI) leaves the environment alone.
    pub fn system() -> Self {
        Self::new(|var| std::env::var(var).ok(), system_keyring())
    }

    /// `env` reads a variable; `keyring` is the store, or why there is none.
    pub fn new(
        env: impl Fn(&str) -> Option<String> + 'static,
        keyring: std::result::Result<Arc<CredentialStore>, String>,
    ) -> Self {
        Self {
            env: Box::new(env),
            keyring,
        }
    }

    /// Why the keyring cannot be used, if it cannot.
    pub fn keyring_unavailable(&self) -> Option<&str> {
        self.keyring.as_ref().err().map(String::as_str)
    }

    /// The secret and where it was found, or `None` when it is nowhere.
    pub fn find(&self, name: SecretName) -> Result<Option<(Secret, Source)>> {
        if let Some(value) = self.read_env(name) {
            return Ok(Some((Secret(value), Source::Environment)));
        }
        let value = self
            .read_keyring(name)
            .map_err(|why| keyring_failure("reading", name, why))?;
        Ok(value.map(|value| (Secret(value), Source::Keyring)))
    }

    /// The secret, or an error saying how to provide it.
    pub fn get(&self, name: SecretName) -> Result<Secret> {
        match self.find(name)? {
            Some((secret, _)) => Ok(secret),
            None => Err(self.not_found(name)),
        }
    }

    /// Where the secret is found, without reading more than needed. The error
    /// is a description safe to print.
    pub fn locate(&self, name: SecretName) -> std::result::Result<Option<Source>, String> {
        if self.read_env(name).is_some() {
            return Ok(Some(Source::Environment));
        }
        Ok(self.read_keyring(name)?.map(|_| Source::Keyring))
    }

    /// Whether the keyring holds the secret, whatever the environment says.
    pub fn in_keyring(&self, name: SecretName) -> Result<bool> {
        self.require_keyring(name)?;
        let value = self
            .read_keyring(name)
            .map_err(|why| keyring_failure("reading", name, why))?;
        Ok(value.is_some())
    }

    /// Stores the secret in the keyring, replacing any previous value.
    pub fn store(&self, name: SecretName, secret: &Secret) -> Result<()> {
        let store = self.require_keyring(name)?;
        entry(store, name)
            .and_then(|entry| entry.set_password(secret.expose()))
            .map_err(|err| keyring_failure("storing", name, describe(&err)))
    }

    /// Removes the secret from the keyring. Returns whether it was there.
    pub fn remove(&self, name: SecretName) -> Result<bool> {
        let store = self.require_keyring(name)?;
        match entry(store, name).and_then(|entry| entry.delete_credential()) {
            Ok(()) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(err) => Err(keyring_failure("removing", name, describe(&err))),
        }
    }

    /// The keyring, or an error saying to use the environment instead.
    pub fn require_keyring(&self, name: SecretName) -> Result<&Arc<CredentialStore>> {
        self.keyring.as_ref().map_err(|why| {
            Error::internal(
                format!("the system keyring is unavailable: {why}"),
                format!(
                    "start a keyring service (Secret Service on Linux), or set {} in the environment instead",
                    name.env_var()
                ),
            )
        })
    }

    fn read_env(&self, name: SecretName) -> Option<String> {
        (self.env)(name.env_var()).filter(|value| !value.is_empty())
    }

    /// The keyring value, `None` when absent or when there is no keyring.
    fn read_keyring(&self, name: SecretName) -> std::result::Result<Option<String>, String> {
        let Ok(store) = &self.keyring else {
            return Ok(None);
        };
        match entry(store, name).and_then(|entry| entry.get_password()) {
            Ok(value) => Ok(Some(value)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(err) => Err(describe(&err)),
        }
    }

    fn not_found(&self, name: SecretName) -> Error {
        let var = name.env_var();
        match &self.keyring {
            Ok(_) => Error::usage(
                format!(
                    "secret `{name}` not found: {var} is not set and the system keyring has no `{name}`"
                ),
                format!("run `modpack auth {name}`, or set {var}"),
            ),
            Err(why) => Error::usage(
                format!(
                    "secret `{name}` not found: {var} is not set and the system keyring is unavailable ({why})"
                ),
                format!("set {var} in the environment"),
            ),
        }
    }
}

fn entry(
    store: &Arc<CredentialStore>,
    name: SecretName,
) -> keyring_core::Result<keyring_core::Entry> {
    store.build(KEYRING_SERVICE, name.as_str(), None)
}

fn keyring_failure(action: &str, name: SecretName, why: String) -> Error {
    Error::internal(
        format!("{action} secret `{name}` in the system keyring failed: {why}"),
        format!(
            "unlock the system keyring and retry, or set {} in the environment instead",
            name.env_var()
        ),
    )
}

/// A description of a keyring error that cannot hold a secret value. Some
/// variants carry the raw value, so only those known to be safe are shown.
fn describe(err: &KeyringError) -> String {
    match err {
        KeyringError::PlatformFailure(_)
        | KeyringError::NoStorageAccess(_)
        | KeyringError::NoEntry
        | KeyringError::BadStoreFormat(_)
        | KeyringError::TooLong(..)
        | KeyringError::Invalid(..)
        | KeyringError::NoDefaultStore
        | KeyringError::NotSupportedByStore(_) => err.to_string(),
        KeyringError::BadEncoding(_) => "the stored value is not valid UTF-8".to_string(),
        KeyringError::BadDataFormat(..) => "the stored value is malformed".to_string(),
        KeyringError::Ambiguous(entries) => {
            format!("{} keyring entries match this secret", entries.len())
        }
        _ => "unexpected keyring error".to_string(),
    }
}

/// The keyring of this platform, or why it cannot be used.
fn system_keyring() -> std::result::Result<Arc<CredentialStore>, String> {
    #[cfg(target_os = "linux")]
    let store = zbus_secret_service_keyring_store::Store::new();
    #[cfg(windows)]
    let store = windows_native_keyring_store::Store::new();
    #[cfg(any(target_os = "linux", windows))]
    let store = store
        .map(|store| -> Arc<CredentialStore> { store })
        .map_err(|err| describe(&err));
    #[cfg(not(any(target_os = "linux", windows)))]
    let store = Err("no keyring support on this platform yet".to_string());

    if let Err(why) = &store {
        tracing::debug!(%why, "system keyring unavailable, secrets come from the environment only");
    }
    store
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::error::ErrorKind;
    use keyring_core::mock;

    const VALUE: &str = "s3cr3t-value-never-shown";

    fn mock_store() -> Arc<CredentialStore> {
        mock::Store::new().unwrap()
    }

    fn secrets(env: &[(&str, &str)], keyring: Option<Arc<CredentialStore>>) -> Secrets {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Secrets::new(
            move |var| env.get(var).cloned(),
            keyring.ok_or_else(|| "no Secret Service".to_string()),
        )
    }

    /// Makes the next call on the keyring entry of `name` fail with `err`.
    fn fail_next(store: &Arc<CredentialStore>, name: SecretName, err: KeyringError) {
        let entry = entry(store, name).unwrap();
        let cred: &mock::Cred = entry.as_any().downcast_ref().unwrap();
        cred.set_error(err);
    }

    #[test]
    fn names_and_variables_match_the_design() {
        let pairs: Vec<_> = SecretName::ALL
            .iter()
            .map(|n| (n.as_str(), n.env_var()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("modrinth-token", "MODPACK_MODRINTH_TOKEN"),
                ("curseforge-upload-token", "MODPACK_CURSEFORGE_UPLOAD_TOKEN"),
                ("curseforge-api-key", "MODPACK_CURSEFORGE_API_KEY"),
            ]
        );
        for name in SecretName::ALL {
            let parsed = <SecretName as clap::ValueEnum>::from_str(name.as_str(), false).unwrap();
            assert_eq!(parsed, name);
        }
    }

    #[test]
    fn environment_comes_before_the_keyring() {
        let store = mock_store();
        let name = SecretName::ModrinthToken;
        entry(&store, name)
            .unwrap()
            .set_password("from-keyring")
            .unwrap();

        let both = secrets(&[(name.env_var(), "from-env")], Some(store.clone()));
        let (secret, source) = both.find(name).unwrap().unwrap();
        assert_eq!((secret.expose(), source), ("from-env", Source::Environment));
        assert_eq!(both.locate(name), Ok(Some(Source::Environment)));

        let keyring_only = secrets(&[], Some(store));
        let (secret, source) = keyring_only.find(name).unwrap().unwrap();
        assert_eq!((secret.expose(), source), ("from-keyring", Source::Keyring));
        assert_eq!(keyring_only.locate(name), Ok(Some(Source::Keyring)));
    }

    #[test]
    fn empty_variable_counts_as_unset() {
        let store = mock_store();
        let name = SecretName::CurseforgeApiKey;
        entry(&store, name)
            .unwrap()
            .set_password("from-keyring")
            .unwrap();
        let secrets = secrets(&[(name.env_var(), "")], Some(store));
        assert_eq!(secrets.get(name).unwrap().expose(), "from-keyring");
    }

    #[test]
    fn keyring_is_read_under_the_service_and_the_secret_name() {
        let store = mock_store();
        let secrets = secrets(&[], Some(store.clone()));
        let name = SecretName::CurseforgeUploadToken;
        secrets.store(name, &Secret::new(VALUE)).unwrap();
        let raw = store
            .build("modpack-cli", "curseforge-upload-token", None)
            .unwrap();
        assert_eq!(raw.get_password().unwrap(), VALUE);
    }

    #[test]
    fn missing_secret_says_how_to_provide_it() {
        let secrets = secrets(&[], Some(mock_store()));
        let name = SecretName::ModrinthToken;
        assert!(secrets.find(name).unwrap().is_none());
        assert_eq!(secrets.locate(name), Ok(None));
        let err = secrets.get(name).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("modrinth-token"));
        assert_eq!(
            err.hint(),
            "run `modpack auth modrinth-token`, or set MODPACK_MODRINTH_TOKEN"
        );
    }

    #[test]
    fn without_keyring_the_environment_still_works() {
        let name = SecretName::CurseforgeApiKey;
        let with_env = secrets(&[(name.env_var(), VALUE)], None);
        assert_eq!(with_env.get(name).unwrap().expose(), VALUE);
        assert_eq!(with_env.keyring_unavailable(), Some("no Secret Service"));

        let without = secrets(&[], None);
        assert_eq!(without.locate(name), Ok(None));
        let err = without.get(name).unwrap_err();
        assert!(
            err.to_string()
                .contains("keyring is unavailable (no Secret Service)")
        );
        assert_eq!(
            err.hint(),
            "set MODPACK_CURSEFORGE_API_KEY in the environment"
        );
        let err = without.store(name, &Secret::new(VALUE)).unwrap_err();
        assert!(err.hint().contains("MODPACK_CURSEFORGE_API_KEY"));
        assert!(without.remove(name).is_err());
    }

    #[test]
    fn remove_takes_the_secret_out_of_the_keyring() {
        let secrets = secrets(&[], Some(mock_store()));
        let name = SecretName::ModrinthToken;
        secrets.store(name, &Secret::new(VALUE)).unwrap();
        assert!(secrets.in_keyring(name).unwrap());
        assert!(secrets.remove(name).unwrap());
        assert!(!secrets.in_keyring(name).unwrap());
        assert!(secrets.find(name).unwrap().is_none());
        assert!(
            !secrets.remove(name).unwrap(),
            "removing twice is not an error"
        );
    }

    #[test]
    fn secret_value_never_shows_in_debug_or_display() {
        let secret = Secret::new(VALUE);
        assert!(!format!("{secret:?}").contains(VALUE));
        assert!(!format!("{secret}").contains(VALUE));
        assert!(!format!("{:?}", Some((secret, Source::Keyring))).contains(VALUE));
    }

    #[test]
    fn secret_value_never_shows_in_errors() {
        let store = mock_store();
        let name = SecretName::ModrinthToken;
        let secrets = secrets(&[], Some(store.clone()));
        let failures = [
            KeyringError::BadEncoding(VALUE.as_bytes().to_vec()),
            KeyringError::BadDataFormat(VALUE.as_bytes().to_vec(), "bad".into()),
            KeyringError::Ambiguous(vec![entry(&store, SecretName::CurseforgeApiKey).unwrap()]),
            KeyringError::NoStorageAccess("locked".into()),
        ];
        // The ambiguous entry holds the value too: its mock shows it in `Debug`.
        secrets
            .store(SecretName::CurseforgeApiKey, &Secret::new(VALUE))
            .unwrap();
        for failure in failures {
            fail_next(&store, name, failure);
            let err = secrets.get(name).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Internal);
            assert!(err.to_string().contains("modrinth-token"));
            for shown in [err.to_string(), format!("{err:?}")] {
                assert!(!shown.contains(VALUE), "value leaked in: {shown}");
            }
        }

        fail_next(
            &store,
            name,
            KeyringError::BadEncoding(VALUE.as_bytes().to_vec()),
        );
        let why = secrets.locate(name).unwrap_err();
        assert!(!why.contains(VALUE));
    }
}
