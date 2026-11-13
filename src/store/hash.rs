//! Hashes of a jar: sha1 and sha512 for the manifest, sha256 for the socle
//! registry and the instance state, all computed in one pass.

use std::io::{self, Read, Write};
use std::path::Path;

use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JarHashes {
    pub sha1: String,
    pub sha512: String,
    pub sha256: String,
    pub size: u64,
}

impl JarHashes {
    /// Whether these are the hashes the manifest records for the jar.
    pub fn matches(&self, expected: &crate::model::manifest::Hashes) -> bool {
        self.sha1 == expected.sha1 && self.sha512 == expected.sha512
    }
}

/// Hashes of a file.
pub fn hash_file(path: &Path) -> io::Result<JarHashes> {
    copy_hashing(&mut std::fs::File::open(path)?, &mut io::sink())
}

/// Copies `reader` into `writer` and returns the hashes of what went through.
pub fn copy_hashing(reader: &mut dyn Read, writer: &mut dyn Write) -> io::Result<JarHashes> {
    let (mut sha1, mut sha512, mut sha256) = (Sha1::new(), Sha512::new(), Sha256::new());
    let mut size = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let chunk = &buf[..n];
        sha1.update(chunk);
        sha512.update(chunk);
        sha256.update(chunk);
        writer.write_all(chunk)?;
        size += n as u64;
    }
    writer.flush()?;
    Ok(JarHashes {
        sha1: hex(&sha1.finalize()),
        sha512: hex(&sha512.finalize()),
        sha256: hex(&sha256.finalize()),
        size,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_of_known_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abc.jar");
        std::fs::write(&path, b"abc").unwrap();
        let hashes = hash_file(&path).unwrap();
        assert_eq!(hashes.sha1, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hashes.sha512,
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(hashes.sha256, crate::model::sha256_bytes(b"abc"));
        assert_eq!(hashes.size, 3);
    }

    #[test]
    fn copy_writes_what_it_hashes() {
        let mut out = Vec::new();
        let hashes = copy_hashing(&mut &b"jar bytes"[..], &mut out).unwrap();
        assert_eq!(out, b"jar bytes");
        assert_eq!(hashes.size, 9);
    }
}
