//! Locates the managed, version-pinned Typst binary, downloading it from the
//! official GitHub release on first use (FR-19). Same philosophy as the bundled
//! Syncthing: we never use a system Typst, so rendering is reproducible.
//!
//! Typst ships its releases as `.tar.xz`. Rather than link an xz decoder into
//! the static musl binary, we shell out to the ubiquitous `curl` (download) and
//! `tar` (extract, `-J` for xz) — the same first-run-only external-tool
//! approach already used for Syncthing.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::output::say;
use crate::paths::TYPST_VERSION;

/// Rust target triple Typst publishes assets for, per architecture.
fn release_target() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64-unknown-linux-musl"),
        "aarch64" => Ok("aarch64-unknown-linux-musl"),
        other => bail!(
            "unsupported architecture '{other}' — ByteFerret v1 supports x86_64 and arm64 Linux"
        ),
    }
}

fn reports_pinned_version(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(TYPST_VERSION))
        .unwrap_or(false)
}

/// Returns a ready-to-run Typst binary of the pinned version, downloading and
/// extracting it if necessary. Idempotent.
pub fn ensure_typst(bin_path: &Path) -> Result<PathBuf> {
    if bin_path.exists() && reports_pinned_version(bin_path) {
        return Ok(bin_path.to_path_buf());
    }

    let target = release_target()?;
    let dir = format!("typst-{target}");
    let tar_name = format!("{dir}.tar.xz");
    let url =
        format!("https://github.com/typst/typst/releases/download/v{TYPST_VERSION}/{tar_name}");

    say(&format!("Downloading Typst v{TYPST_VERSION} ({target})…"));
    let bin_dir = bin_path.parent().context("bin path has no parent")?;
    fs::create_dir_all(bin_dir)?;
    let stage = bin_dir.join(".typst-download");
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
        bail!("failed to download Typst from {url} (curl exit {status})");
    }

    // Extract with system tar (xz): the binary lands at `<dir>/typst`.
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&stage)
        .status()
        .context("failed to run tar (is it installed?)")?;
    if !status.success() {
        bail!(
            "failed to extract {} (tar exit {status})",
            tar_path.display()
        );
    }

    let extracted = stage.join(&dir).join("typst");
    if !extracted.exists() {
        bail!("Typst archive did not contain `{dir}/typst`");
    }
    fs::copy(&extracted, bin_path)
        .with_context(|| format!("installing typst to {}", bin_path.display()))?;
    fs::set_permissions(bin_path, fs::Permissions::from_mode(0o755))?;
    let _ = fs::remove_dir_all(&stage);

    if !reports_pinned_version(bin_path) {
        bail!("downloaded Typst does not report {TYPST_VERSION}");
    }
    say(&format!("Typst v{TYPST_VERSION} ready."));
    Ok(bin_path.to_path_buf())
}
