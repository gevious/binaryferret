//! `binaryferret service install|uninstall|status` — manage the systemd user
//! service (FR-1). All systemd interaction is confined here; the unit-file
//! content itself lives in `crate::service` (testable in isolation).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::output::{emit, say};
use crate::paths::systemd_user_unit;
use crate::service::{unit_contents, UNIT_NAME};

/// Absolute path of the running binaryferret binary, for baking into the unit.
fn self_exe() -> Result<String> {
    let exe = std::env::current_exe().context("locating the binaryferret binary")?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    Ok(exe.to_string_lossy().to_string())
}

/// Run `systemctl --user <args>`, returning stdout on success. A missing
/// `systemctl` (or absent user bus) is reported as an actionable error.
fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow::anyhow!(
                    "`systemctl` not found — this system does not appear to use systemd"
                )
            }
            _ => anyhow::anyhow!("running systemctl: {e}"),
        })?;
    if !out.status.success() {
        bail!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Same as `systemctl` but tolerates a non-zero exit (used for query verbs like
/// `is-active` where "inactive" is a normal, expected answer, not an error).
fn systemctl_query(args: &[&str]) -> String {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn install(now: bool) -> Result<()> {
    let exe = self_exe()?;
    let unit_path: PathBuf = systemd_user_unit();
    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&unit_path, unit_contents(&exe))
        .with_context(|| format!("writing {}", unit_path.display()))?;

    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", UNIT_NAME])?;
    if now {
        systemctl(&["start", UNIT_NAME])?;
    }

    say(&format!(
        "Installed systemd user service → {}",
        unit_path.display()
    ));
    say(&format!(
        "  enabled: starts automatically on login{}",
        if now { ", started now" } else { "" }
    ));
    say("");
    say("If this machine is a headless server (no interactive login), enable lingering");
    say("so the agent runs without you being logged in:");
    say("  sudo loginctl enable-linger \"$USER\"");
    emit(&json!({
        "ok": true, "action": "install", "unit": unit_path.to_string_lossy(),
        "enabled": true, "startedNow": now,
    }));
    Ok(())
}

pub fn uninstall() -> Result<()> {
    // Best-effort stop+disable; ignore failures so an already-partly-removed
    // service still cleans up its unit file.
    let _ = systemctl(&["disable", "--now", UNIT_NAME]);
    let unit_path = systemd_user_unit();
    let removed = unit_path.exists();
    if removed {
        fs::remove_file(&unit_path).with_context(|| format!("removing {}", unit_path.display()))?;
    }
    let _ = systemctl(&["daemon-reload"]);

    say(if removed {
        "Removed the systemd user service."
    } else {
        "No systemd user service was installed."
    });
    emit(&json!({ "ok": true, "action": "uninstall", "removed": removed }));
    Ok(())
}

pub fn status() -> Result<()> {
    let unit_path = systemd_user_unit();
    let installed = unit_path.exists();
    let active = systemctl_query(&["is-active", UNIT_NAME]);
    let enabled = systemctl_query(&["is-enabled", UNIT_NAME]);

    say(&format!(
        "service:  {}",
        if installed {
            unit_path.display().to_string()
        } else {
            "not installed".into()
        }
    ));
    if installed {
        say(&format!(
            "active:   {}",
            if active.is_empty() {
                "unknown".into()
            } else {
                active.clone()
            }
        ));
        say(&format!(
            "enabled:  {}",
            if enabled.is_empty() {
                "unknown".into()
            } else {
                enabled.clone()
            }
        ));
    } else {
        say("install with: binaryferret service install --now");
    }
    emit(&json!({
        "ok": true, "action": "status", "installed": installed,
        "active": active, "enabled": enabled,
    }));
    Ok(())
}
