//! Formats of the repository and of the machine: the types every command
//! reads and writes, and nothing that talks to the network.

pub mod installed;
pub mod instance;
pub mod manifest;
pub mod socle;
pub mod variants;

use std::fs;
use std::io::Read;
use std::path::Path;

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::error::{Context, Error, Result};

/// Hex-encoded sha256 of a file's content.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Hex-encoded sha256 of bytes.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses a TOML file of the repository; a malformed one is the user's to fix.
fn parse_toml<T: DeserializeOwned>(text: &str, name: &str, hint: &str) -> Result<T> {
    toml::from_str(text).map_err(|e| {
        Error::usage(format!("{name} is invalid: {}", e.message()), hint).with_source(e)
    })
}

fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).context(format!("reading {}", path.display()))
}

/// Writes through a temporary file then a rename, so that a crash never
/// leaves a half-written file.
fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(format!("creating {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, text).context(format!("writing {}", path.display()))?;
    fs::rename(&tmp, path).context(format!("writing {}", path.display()))
}

/// Serializes a value the CLI generated; failing here is a bug.
fn to_toml<T: serde::Serialize>(value: &T) -> String {
    toml::to_string(value).expect("model types always serialize to TOML")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_file_and_bytes_agree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"abc").unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&path).unwrap(), expected);
        assert_eq!(sha256_bytes(b"abc"), expected);
    }
}
