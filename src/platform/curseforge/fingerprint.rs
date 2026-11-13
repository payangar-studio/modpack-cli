//! CurseForge file fingerprint: 32-bit MurmurHash2, seed 1, over the file's
//! bytes once tab, line feed, carriage return and space bytes are removed.
//!
//! The filter and the seed follow packwiz (`curseforge/murmur2/hash.go`),
//! which hashes with `github.com/aviddiviner/go-murmur`; the hash itself is
//! Austin Appleby's original MurmurHash2.

const M: u32 = 0x5bd1_e995;
const R: u32 = 24;

/// Fingerprint of a file as CurseForge computes it.
pub fn fingerprint(bytes: &[u8]) -> u32 {
    let kept: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !matches!(b, 9 | 10 | 13 | 32))
        .collect();
    murmur2(&kept, 1)
}

/// MurmurHash2, 32 bits, as in SMHasher's `MurmurHash2.cpp`.
fn murmur2(data: &[u8], seed: u32) -> u32 {
    // The length is folded into the seed on 32 bits, as the reference does.
    let mut h = seed ^ data.len() as u32;
    let (chunks, tail) = data.as_chunks::<4>();
    for chunk in chunks {
        let mut k = u32::from_le_bytes(*chunk);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
    }
    if tail.len() == 3 {
        h ^= u32::from(tail[2]) << 16;
    }
    if tail.len() >= 2 {
        h ^= u32::from(tail[1]) << 8;
    }
    if !tail.is_empty() {
        h ^= u32::from(tail[0]);
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^ (h >> 15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Values from the tests of go-murmur, the library packwiz hashes with.
    #[test]
    fn murmur2_matches_go_murmur() {
        assert_eq!(murmur2(b"foo", 123), 1_412_061_192);
        assert_eq!(murmur2(b"zztop", 123), 1_878_194_508);
        assert_eq!(murmur2(b"foobarbaz", 234), 1_777_016_281);
        assert_eq!(murmur2(b"blam", 777), 1_668_928_339);
    }

    /// SMHasher's verification test: hash keys {}, {0}, {0, 1}, ... with
    /// seed 256 - length, then hash the concatenated results with seed 0.
    /// The expected value is the one SMHasher lists for MurmurHash2.
    #[test]
    fn murmur2_passes_smhasher_verification() {
        let key: Vec<u8> = (0..=255).collect();
        let mut hashes = Vec::new();
        for len in 0..256 {
            hashes.extend(murmur2(&key[..len], 256 - len as u32).to_le_bytes());
        }
        assert_eq!(murmur2(&hashes, 0), 0x2786_4c1e);
    }

    #[test]
    fn whitespace_bytes_are_ignored() {
        let spaced = b"line one\twith tab\r\n  indented line\n\nend \n";
        assert_eq!(
            fingerprint(spaced),
            fingerprint(b"lineonewithtabindentedlineend")
        );
        assert_eq!(fingerprint(spaced), 389_384_341);
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }

    /// Expected value computed with a transcription of go-murmur plus
    /// packwiz's whitespace filter, checked against the vectors above.
    #[test]
    fn fixture_jar() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/curseforge/fixture-1.0.0.jar");
        assert_eq!(fingerprint(&std::fs::read(path).unwrap()), 550_359_374);
    }
}
