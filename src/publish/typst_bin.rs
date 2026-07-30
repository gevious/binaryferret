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

use crate::fetch::download_verified;
use crate::output::say;
use crate::paths::TYPST_VERSION;

/// Rust target triple Typst publishes assets for, per OS + architecture. Linux
/// uses the static musl builds; macOS uses the Apple/Darwin builds. Both ship as
/// `.tar.xz`, so the extraction path below is identical.
fn release_target() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported architecture '{other}' — ByteFerret supports x86_64 and arm64"),
    };
    let suffix = match std::env::consts::OS {
        "linux" => "unknown-linux-musl",
        "macos" => "apple-darwin",
        other => bail!("unsupported OS '{other}' — ByteFerret supports Linux and macOS"),
    };
    Ok(format!("{arch}-{suffix}"))
}

/// SHA-256 of each pinned Typst release archive. Typst does not publish a
/// checksum file, so these were computed from the v0.12.0 release assets and
/// vendored here: they pin the artifact to exactly what that release served, so
/// a later substitution upstream or in transit is caught. Recompute all four
/// (`sha256sum typst-<target>.tar.xz`) whenever `TYPST_VERSION` changes.
///
/// An unlisted target fails closed rather than installing something unchecked.
fn release_sha256(target: &str) -> Result<&'static str> {
    Ok(match target {
        "x86_64-unknown-linux-musl" => {
            "605130a770ebd59a4a579673079cb913a13e75985231657a71d6239a57539ec3"
        }
        "aarch64-unknown-linux-musl" => {
            "e81ae98e6b12db5a36c2276e5a9890da48f7a339b92476dd22daf90de3699e11"
        }
        "x86_64-apple-darwin" => {
            "cba1b1e43b992603da9cc7777ac1c8af2c413f381453169a22c57d9a9aa02efb"
        }
        "aarch64-apple-darwin" => {
            "0e7e7b370b240dab104654bad1ceffdbbcb17b1c98ee8b7778360ddad479fb5d"
        }
        _ => bail!(
            "no pinned checksum for Typst v{TYPST_VERSION} on {target} — \
             add it before this target can be used"
        ),
    })
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

    download_verified(&url, &tar_path, release_sha256(&target)?)?;

    // Extract with system tar (xz), naming the one member we want rather than
    // unpacking the whole archive: the binary lands at `<dir>/typst`.
    let member = format!("{dir}/typst");
    let status = Command::new("tar")
        .arg("-xJf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&stage)
        .arg(&member)
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
