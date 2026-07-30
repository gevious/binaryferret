//! `byteferret service install|uninstall|status` — manage the user service
//! (FR-1): a systemd user unit on Linux, a launchd user agent on macOS. All
//! init-system interaction is confined here; the unit/plist content itself
//! lives in `crate::service` (testable in isolation).

use std::fs;

use anyhow::{Context, Result};

/// Absolute path of the running byteferret binary, for baking into the unit.
fn self_exe() -> Result<String> {
    let exe = std::env::current_exe().context("locating the byteferret binary")?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    Ok(exe.to_string_lossy().to_string())
}

#[cfg(not(target_os = "macos"))]
pub use systemd::{install, status, uninstall};

#[cfg(target_os = "macos")]
pub use launchd::{install, status, uninstall};

#[cfg(not(target_os = "macos"))]
mod systemd {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    use anyhow::{bail, Context, Result};
    use serde_json::json;

    use super::self_exe;
    use crate::output::{emit, say};
    use crate::paths::systemd_user_unit;
    use crate::service::{unit_contents, UNIT_NAME};

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
            fs::remove_file(&unit_path)
                .with_context(|| format!("removing {}", unit_path.display()))?;
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
            say("install with: byteferret service install --now");
        }
        emit(&json!({
            "ok": true, "action": "status", "installed": installed,
            "active": active, "enabled": enabled,
        }));
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod launchd {
    use std::fs;
    use std::process::Command;

    use anyhow::{bail, Context, Result};
    use serde_json::json;

    use super::self_exe;
    use crate::output::{emit, say};
    use crate::paths::{launchd_agent_plist, Paths};
    use crate::service::{plist_contents, LAUNCHD_LABEL};

    /// The per-user launchd domain of the calling user, e.g. `gui/501`.
    fn gui_domain() -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    /// Run `launchctl <args>`, failing with the command's stderr on error.
    fn launchctl(args: &[&str]) -> Result<()> {
        let out = Command::new("launchctl")
            .args(args)
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    anyhow::anyhow!("`launchctl` not found — is this macOS?")
                }
                _ => anyhow::anyhow!("running launchctl: {e}"),
            })?;
        if !out.status.success() {
            bail!(
                "launchctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Whether launchd currently has the agent loaded in the user's domain.
    fn is_loaded() -> bool {
        Command::new("launchctl")
            .args(["print", &format!("{}/{LAUNCHD_LABEL}", gui_domain())])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn install(now: bool) -> Result<()> {
        let exe = self_exe()?;
        let plist_path = launchd_agent_plist();
        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&plist_path, plist_contents(&exe))
            .with_context(|| format!("writing {}", plist_path.display()))?;

        // A plist in ~/Library/LaunchAgents is bootstrapped automatically at the
        // next login; bootstrapping it now also fires RunAtLoad (= start now).
        if now {
            // Unload any previous copy first so a reinstall picks up the new plist.
            let _ = launchctl(&["bootout", &format!("{}/{LAUNCHD_LABEL}", gui_domain())]);
            launchctl(&["bootstrap", &gui_domain(), &plist_path.to_string_lossy()])?;
        }

        say(&format!(
            "Installed launchd user agent → {}",
            plist_path.display()
        ));
        say(&format!(
            "  enabled: starts automatically on login{}",
            if now { ", started now" } else { "" }
        ));
        emit(&json!({
            "ok": true, "action": "install", "unit": plist_path.to_string_lossy(),
            "enabled": true, "startedNow": now,
        }));
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        // Best-effort unload; ignore failures so an already-partly-removed agent
        // still cleans up its plist.
        let _ = launchctl(&["bootout", &format!("{}/{LAUNCHD_LABEL}", gui_domain())]);
        let plist_path = launchd_agent_plist();
        let removed = plist_path.exists();
        if removed {
            fs::remove_file(&plist_path)
                .with_context(|| format!("removing {}", plist_path.display()))?;
        }
        // launchd has no ExecStop hook, so mirror the systemd `disable --now`
        // behavior (which runs `byteferret stop`) by stopping Syncthing directly.
        let _ = crate::syncthing::process::stop(&Paths::resolve());

        say(if removed {
            "Removed the launchd user agent."
        } else {
            "No launchd user agent was installed."
        });
        emit(&json!({ "ok": true, "action": "uninstall", "removed": removed }));
        Ok(())
    }

    pub fn status() -> Result<()> {
        let plist_path = launchd_agent_plist();
        let installed = plist_path.exists();
        let loaded = is_loaded();
        // Keep the JSON keys aligned with the Linux output so callers don't
        // have to branch per OS. A plist in ~/Library/LaunchAgents is what
        // "enabled" means on macOS; "active" is whether launchd has it loaded.
        let active = if loaded { "active" } else { "inactive" };
        let enabled = if installed { "enabled" } else { "disabled" };

        say(&format!(
            "service:  {}",
            if installed {
                plist_path.display().to_string()
            } else {
                "not installed".into()
            }
        ));
        if installed {
            say(&format!("active:   {active}"));
            say(&format!("enabled:  {enabled}"));
        } else {
            say("install with: byteferret service install --now");
        }
        emit(&json!({
            "ok": true, "action": "status", "installed": installed,
            "active": active, "enabled": enabled,
        }));
        Ok(())
    }
}
