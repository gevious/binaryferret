//! Locates the managed, version-pinned Syncthing binary, downloading it from
//! the official GitHub release on first use (FR-2). We deliberately do NOT use
//! any system-installed Syncthing so behavior is reproducible across machines.
//!
//! The HTTPS download uses `curl` (a documented dependency present on target
//! machines) so the agent binary itself needs no TLS stack — keeping the static
//! musl build free of OpenSSL/ring.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use tar::Archive;

use crate::output::say;
use crate::paths::SYNCTHING_VERSION;

fn release_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => bail!("unsupported architecture '{other}' — BinaryFerret v1 supports x86_64 and arm64 Linux"),
    }
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
    let dir = format!("syncthing-linux-{arch}-v{SYNCTHING_VERSION}");
    let tar_name = format!("{dir}.tar.gz");
    let url = format!("https://github.com/syncthing/syncthing/releases/download/v{SYNCTHING_VERSION}/{tar_name}");

    say(&format!("Downloading Syncthing v{SYNCTHING_VERSION} ({arch})…"));
    let bin_dir = bin_path.parent().context("bin path has no parent")?;
    fs::create_dir_all(bin_dir)?;
    let stage = bin_dir.join(".download");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;
    let tar_path = stage.join(&tar_name);

    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "300", "-o"])
        .arg(&tar_path)
        .arg(&url)
        .status()
        .context("failed to run curl (is it installed?)")?;
    if !status.success() {
        bail!("failed to download Syncthing from {url} (curl exit {status})");
    }

    // The release tarball also ships example rc/init scripts named `syncthing`;
    // match the real binary by its exact top-level path.
    let expected_rel = format!("{dir}/syncthing");
    extract_syncthing(&tar_path, bin_path, &expected_rel)
        .with_context(|| format!("extracting {}", tar_path.display()))?;
    fs::set_permissions(bin_path, fs::Permissions::from_mode(0o755))?;
    let _ = fs::remove_dir_all(&stage);

    if !reports_pinned_version(bin_path) {
        bail!("downloaded Syncthing does not report v{SYNCTHING_VERSION}");
    }
    say(&format!("Syncthing v{SYNCTHING_VERSION} ready."));
    Ok(bin_path.to_path_buf())
}

/// Extract the `syncthing` binary (at `expected_rel` inside the archive) to `dest`.
fn extract_syncthing(tar_path: &Path, dest: &Path, expected_rel: &str) -> Result<()> {
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
