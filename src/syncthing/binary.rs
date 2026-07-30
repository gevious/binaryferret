//! Locates the managed, version-pinned Syncthing binary, downloading it from
//! the official GitHub release on first use (FR-2). We deliberately do NOT use
//! any system-installed Syncthing so behavior is reproducible across machines.
//!
//! The HTTPS download uses `curl` (a documented dependency present on target
//! machines) so the agent binary itself needs no TLS stack — keeping the static
//! musl build free of OpenSSL/ring — so the archive's pinned SHA-256, not the
//! transport, is what establishes we got what upstream published (see `fetch`).
//! On Linux the release is a `.tar.gz` we
//! decompress in-process; on macOS it is a `.zip`, extracted by shelling out to
//! the system `unzip` (same first-run-only external-tool approach as `curl`).

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use tar::Archive;

use crate::fetch::download_verified;
use crate::output::say;
use crate::paths::SYNCTHING_VERSION;

fn release_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => bail!("unsupported architecture '{other}' — ByteFerret supports x86_64 and arm64"),
    }
}

/// Syncthing's per-OS release naming: Linux tarballs are `.tar.gz`, macOS builds
/// ship as `.zip`. Returns (os-slug-used-in-the-asset-name, archive-extension).
fn release_os() -> Result<(&'static str, &'static str)> {
    match std::env::consts::OS {
        "linux" => Ok(("linux", "tar.gz")),
        "macos" => Ok(("macos", "zip")),
        other => bail!("unsupported OS '{other}' — ByteFerret supports Linux and macOS"),
    }
}

/// SHA-256 of each pinned Syncthing release archive, taken from the project's
/// PGP-signed `sha256sum.txt.asc` for this version. Re-derive all four from that
/// file whenever `SYNCTHING_VERSION` changes.
///
/// An unlisted target fails closed: better to refuse to install than to install
/// something we cannot check. The `--version` probe below is not a substitute —
/// a substituted binary can print whatever version string it likes.
fn release_sha256(os: &str, arch: &str) -> Result<&'static str> {
    Ok(match (os, arch) {
        ("linux", "amd64") => "a2edc833d6dd71a76b7c481dbcc81f8be37da9eb83993c512bf53eb096ba7a73",
        ("linux", "arm64") => "4655e260e94fa5e0110084040751bd0274acdeb74653933f909036e788a911a1",
        ("macos", "amd64") => "eb375302c79b89b85f32f014e451430efe3a9723b37639698310344a9029799e",
        ("macos", "arm64") => "dd42cc7a88d08779c305e5f6f2d8bc8dec2c97652c1380679c672d4febd63f8f",
        _ => bail!(
            "no pinned checksum for Syncthing v{SYNCTHING_VERSION} on {os}-{arch} — \
             add it from the release's sha256sum.txt.asc before this target can be used"
        ),
    })
}

/// Verify the binary at `path` reports the pinned version.
fn reports_pinned_version(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("v{SYNCTHING_VERSION}")))
        .unwrap_or(false)
}

/// Returns the path to a ready-to-run Syncthing binary of the pinned version,
/// downloading and extracting it if necessary. Idempotent.
pub fn ensure_binary(bin_path: &Path) -> Result<PathBuf> {
    if bin_path.exists() && reports_pinned_version(bin_path) {
        return Ok(bin_path.to_path_buf());
    }

    let arch = release_arch()?;
    let (os, ext) = release_os()?;
    let dir = format!("syncthing-{os}-{arch}-v{SYNCTHING_VERSION}");
    let asset = format!("{dir}.{ext}");
    let url = format!("https://github.com/syncthing/syncthing/releases/download/v{SYNCTHING_VERSION}/{asset}");

    say(&format!("Downloading Syncthing v{SYNCTHING_VERSION} ({os}-{arch})…"));
    let bin_dir = bin_path.parent().context("bin path has no parent")?;
    fs::create_dir_all(bin_dir)?;
    let stage = bin_dir.join(".download");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;
    let archive_path = stage.join(&asset);

    download_verified(&url, &archive_path, release_sha256(os, arch)?)?;

    // The archive also ships example rc/init scripts named `syncthing`; match the
    // real binary by its exact top-level path inside the archive.
    let expected_rel = format!("{dir}/syncthing");
    match ext {
        "zip" => extract_syncthing_zip(&archive_path, bin_path, &stage, &expected_rel),
        _ => extract_syncthing_tar(&archive_path, bin_path, &expected_rel),
    }
    .with_context(|| format!("extracting {}", archive_path.display()))?;
    fs::set_permissions(bin_path, fs::Permissions::from_mode(0o755))?;
    let _ = fs::remove_dir_all(&stage);

    if !reports_pinned_version(bin_path) {
        bail!("downloaded Syncthing does not report v{SYNCTHING_VERSION}");
    }
    say(&format!("Syncthing v{SYNCTHING_VERSION} ready."));
    Ok(bin_path.to_path_buf())
}

/// Extract the `syncthing` binary (at `expected_rel` inside a `.tar.gz`) to `dest`.
fn extract_syncthing_tar(tar_path: &Path, dest: &Path, expected_rel: &str) -> Result<()> {
    let file = File::open(tar_path)?;
    let mut archive = Archive::new(GzDecoder::new(file));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.to_string_lossy() == expected_rel {
            entry.unpack(dest)?;
            return Ok(());
        }
    }
    bail!("archive did not contain `{expected_rel}`")
}

/// Extract the `syncthing` binary from a macOS `.zip`. Rather than link a zip
/// decoder into the binary we shell out to `unzip` (ships with macOS), matching
/// the first-run-only external-tool approach used for the `curl` download. The
/// wanted entry (`expected_rel`) is unpacked under `stage` and moved to `dest`.
fn extract_syncthing_zip(zip_path: &Path, dest: &Path, stage: &Path, expected_rel: &str) -> Result<()> {
    let status = Command::new("unzip")
        .args(["-o", "-q"])
        .arg(zip_path)
        .arg(expected_rel)
        .arg("-d")
        .arg(stage)
        .status()
        .context("failed to run unzip (is it installed?)")?;
    if !status.success() {
        bail!("unzip failed for {} (exit {status})", zip_path.display());
    }
    let extracted = stage.join(expected_rel);
    if !extracted.exists() {
        bail!("archive did not contain `{expected_rel}`");
    }
    fs::copy(&extracted, dest)
        .with_context(|| format!("installing syncthing to {}", dest.display()))?;
    Ok(())
}
