//! Where the Modrinth App keeps its instances.
//!
//! Read in the app's code (`app-lib`, `state/dirs.rs`): its settings directory
//! is `THESEUS_CONFIG_DIR`, or `<data dir>/ModrinthApp`; it holds `app.db`.
//! Instances live in `profiles/` of the app directory, which is the settings
//! directory unless the "app directory" setting moved it (column `custom_dir`
//! of the `settings` table in `app.db`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::error::{Error, Result};

/// Overrides the app directory (the one holding `profiles/`).
pub const DIR_ENV: &str = "MODPACK_MODRINTH_DIR";
/// The app's own override of its settings directory.
const APP_ENV: &str = "THESEUS_CONFIG_DIR";
/// Name of the app's settings directory under the user's data directory.
const APP_IDENTIFIER: &str = "ModrinthApp";
const PROFILES: &str = "profiles";
const DATABASE: &str = "app.db";

/// The app's `profiles/` directory on this machine.
///
/// `dir`, when given (a command-line argument), then [`DIR_ENV`], replace the
/// app directory and skip `app.db`. Otherwise the app directory is found as
/// the app finds it, reading its "app directory" setting from `app.db`
/// without ever writing to it.
pub fn profiles_dir(dir: Option<&Path>) -> Result<PathBuf> {
    let data_dir = directories::BaseDirs::new().map(|d| d.data_dir().to_owned());
    resolve(dir, |name| std::env::var_os(name), data_dir)
}

fn resolve(
    dir: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
    data_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    let env = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = dir.map(Path::to_owned).or_else(|| env(DIR_ENV)) {
        return Ok(dir.join(PROFILES));
    }
    let settings_dir = env(APP_ENV)
        .or_else(|| data_dir.map(|d| d.join(APP_IDENTIFIER)))
        .ok_or_else(|| {
            Error::internal(
                "cannot find the user's data directory, where the Modrinth App keeps its data",
                format!("set {DIR_ENV} to the Modrinth App directory"),
            )
        })?;
    let app_dir = custom_dir(&settings_dir.join(DATABASE)).unwrap_or(settings_dir);
    Ok(app_dir.join(PROFILES))
}

/// The "app directory" setting, when set. A database that cannot be read is
/// reported and ignored: the default directory is then the best guess.
fn custom_dir(db: &Path) -> Option<PathBuf> {
    if !db.exists() {
        return None;
    }
    match read_custom_dir(db) {
        Ok(dir) => dir.filter(|d| !d.is_empty()).map(PathBuf::from),
        Err(e) => {
            tracing::warn!(
                "cannot read the Modrinth App directory setting from {}: {e}; \
                 if the app directory was moved, set {DIR_ENV} to it",
                db.display()
            );
            None
        }
    }
}

fn read_custom_dir(db: &Path) -> rusqlite::Result<Option<String>> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn
        .query_row("SELECT custom_dir FROM settings LIMIT 1", [], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(vars: &[(&str, &Path)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.as_os_str().to_owned()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    /// A fake `app.db` with the app's `settings` table reduced to its column,
    /// in WAL mode like the app's. Holding the connection stands for the app
    /// running.
    fn app_db(settings_dir: &Path, custom_dir: Option<&Path>) -> Connection {
        std::fs::create_dir_all(settings_dir).unwrap();
        let conn = Connection::open(settings_dir.join(DATABASE)).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute("CREATE TABLE settings (custom_dir TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO settings (custom_dir) VALUES (?1)",
            [custom_dir.map(|d| d.to_str().unwrap())],
        )
        .unwrap();
        conn
    }

    #[test]
    fn defaults_to_the_data_dir() {
        let data = tempfile::tempdir().unwrap();
        let found = resolve(None, env(&[]), Some(data.path().to_owned())).unwrap();
        assert_eq!(found, data.path().join("ModrinthApp/profiles"));
    }

    #[test]
    fn follows_the_apps_own_override() {
        let data = tempfile::tempdir().unwrap();
        let settings = data.path().join("elsewhere");
        let found = resolve(
            None,
            env(&[(APP_ENV, &settings)]),
            Some(data.path().to_owned()),
        )
        .unwrap();
        assert_eq!(found, settings.join("profiles"));
    }

    #[test]
    fn cli_overrides_win_and_skip_the_database() {
        let data = tempfile::tempdir().unwrap();
        let from_env = data.path().join("env");
        let from_arg = data.path().join("arg");
        app_db(&data.path().join("ModrinthApp"), Some(Path::new("/moved")));
        let vars = [(DIR_ENV, from_env.as_path())];
        let data_dir = Some(data.path().to_owned());

        let found = resolve(Some(&from_arg), env(&vars), data_dir.clone()).unwrap();
        assert_eq!(found, from_arg.join("profiles"));
        let found = resolve(None, env(&vars), data_dir).unwrap();
        assert_eq!(found, from_env.join("profiles"));
    }

    #[test]
    fn reads_the_moved_app_directory_from_the_database() {
        let data = tempfile::tempdir().unwrap();
        let moved = data.path().join("games/modrinth");
        let _app = app_db(&data.path().join("ModrinthApp"), Some(&moved));
        assert!(data.path().join("ModrinthApp/app.db-wal").exists());
        let found = resolve(None, env(&[]), Some(data.path().to_owned())).unwrap();
        assert_eq!(found, moved.join("profiles"));
    }

    #[test]
    fn unset_setting_keeps_the_default() {
        let data = tempfile::tempdir().unwrap();
        app_db(&data.path().join("ModrinthApp"), None);
        let found = resolve(None, env(&[]), Some(data.path().to_owned())).unwrap();
        assert_eq!(found, data.path().join("ModrinthApp/profiles"));
    }

    #[test]
    fn unreadable_database_keeps_the_default() {
        let data = tempfile::tempdir().unwrap();
        let settings = data.path().join("ModrinthApp");
        std::fs::create_dir_all(&settings).unwrap();
        std::fs::write(settings.join(DATABASE), b"not a database").unwrap();
        let found = resolve(None, env(&[]), Some(data.path().to_owned())).unwrap();
        assert_eq!(found, settings.join("profiles"));
        assert_eq!(
            std::fs::read(settings.join(DATABASE)).unwrap(),
            b"not a database"
        );
    }

    #[test]
    fn no_data_dir_asks_for_the_override() {
        let err = resolve(None, env(&[]), None).unwrap_err();
        assert!(err.hint().contains(DIR_ENV));
    }
}
