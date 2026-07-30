//! Downloading a pinned release asset, with its integrity checked before use.
//!
//! Both managed binaries (Syncthing, Typst) are fetched from a GitHub release on
//! first run. The download itself shells out to `curl` so the agent needs no TLS
//! stack of its own — but that also means the agent never sees the TLS session,
//! so the pinned SHA-256 is what actually establishes that the bytes we are about
//! to make executable are the ones upstream published. Verifying *before*
//! extraction keeps a substituted archive away from the tar/zip readers, too.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `path`, streamed so a large archive never has to be
/// held in memory.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download `url` to `dest`, failing unless the result hashes to `expected_sha256`.
/// A file that does not match is deleted, never left on disk for a later run to
/// pick up.
pub fn download_verified(url: &str, dest: &Path, expected_sha256: &str) -> Result<()> {
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "300", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .context("failed to run curl (is it installed?)")?;
    if !status.success() {
        bail!("failed to download {url} (curl exit {status})");
    }

    let actual = sha256_file(dest)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = std::fs::remove_file(dest);
        bail!(
            "checksum mismatch for {url}\n  expected {expected_sha256}\n  actual   {actual}\n\
             The download was discarded. What was served does not match the pinned release, so \
             it was not extracted or run."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sha256_file;
    use std::io::Write;

    /// The empty-string and "abc" digests are the standard FIPS 180-4 vectors —
    /// they confirm the file is being hashed as-is, with no stray newline.
    #[test]
    fn hashes_match_known_vectors() {
        let dir = std::env::temp_dir().join(format!("byteferret-sha-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let empty = dir.join("empty");
        std::fs::File::create(&empty).unwrap();
        assert_eq!(
            sha256_file(&empty).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let abc = dir.join("abc");
        std::fs::File::create(&abc).unwrap().write_all(b"abc").unwrap();
        assert_eq!(
            sha256_file(&abc).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        // Larger than the 64 KiB read buffer, to exercise multi-chunk updates.
        let big = dir.join("big");
        std::fs::File::create(&big).unwrap().write_all(&vec![b'a'; 200_000]).unwrap();
        assert_eq!(sha256_file(&big).unwrap().len(), 64);

        std::fs::remove_dir_all(&dir).ok();
    }
}
