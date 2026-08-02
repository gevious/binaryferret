//! Local, user-chosen labels for device ids.
//!
//! An alias is stored on this machine only, in `config.toml`. It is a different
//! thing from a peer's Syncthing `name`, which the *remote* machine chooses and
//! which pairing therefore refuses to match on (a stranger could name itself
//! `laptop`). Because an alias is set locally, it is trusted: once set, it can
//! stand in for a device id anywhere a target is named — `pair --with <alias>`,
//! `pair <alias> --accept`, and so on — and `status` shows it in place of the
//! bare id.

use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{load_context, normalize_id, resolve_device_target, Context};
use crate::output::{emit, sanitize, say};

/// Length (dashes/case ignored) at or above which a target is treated as a
/// complete device id rather than a prefix — so an id copied from another
/// machine can be aliased before this machine has ever paired with it.
const MIN_FULL_ID_LEN: usize = 50;

/// `byteferret alias [<device-id> [<alias>]] [--remove]`.
///  - no device            → list every alias set here
///  - device + alias        → set (or replace) the alias
///  - device + --remove     → clear the alias
///  - device only           → report what's set for it
pub fn alias(device: Option<&str>, new_alias: Option<&str>, remove: bool) -> Result<()> {
    let mut ctx = load_context()?;

    let Some(device) = device.map(str::trim).filter(|s| !s.is_empty()) else {
        return list(&ctx);
    };

    let device_id = resolve(&ctx, device)?;

    if remove {
        let removed = ctx.config.remove_alias(&device_id);
        ctx.config.save(&ctx.paths)?;
        if removed {
            say(&format!("Removed the alias for {device_id}."));
        } else {
            say(&format!("{} has no alias to remove.", short(&device_id)));
        }
        emit(&json!({ "ok": true, "action": "remove", "deviceId": device_id, "removed": removed }));
        return Ok(());
    }

    let Some(new_alias) = new_alias.map(str::trim) else {
        // `alias <device>` alone: report the current mapping.
        match ctx.config.alias_for(&device_id) {
            Some(a) => say(&format!("{} → {}", sanitize(a), device_id)),
            None => say(&format!(
                "{} has no alias. Set one: byteferret alias {} <alias>",
                short(&device_id),
                short(&device_id)
            )),
        }
        emit(&json!({ "ok": true, "deviceId": device_id, "alias": ctx.config.alias_for(&device_id) }));
        return Ok(());
    };

    if new_alias.is_empty() {
        bail!("an alias cannot be empty — pass a label, or use --remove to clear one.");
    }
    // Keep aliases unique across devices so a target like `pair --with <alias>`
    // resolves to exactly one machine.
    if let Some(other) = ctx.config.device_for_alias(new_alias) {
        if normalize_id(other) != normalize_id(&device_id) {
            bail!("alias '{}' is already used by {other} — pick another.", sanitize(new_alias));
        }
    }
    ctx.config.set_alias(device_id.clone(), new_alias.to_string());
    ctx.config.save(&ctx.paths)?;
    say(&format!("Aliased {device_id} as '{}'.", sanitize(new_alias)));
    emit(&json!({ "ok": true, "action": "set", "deviceId": device_id, "alias": new_alias }));
    Ok(())
}

/// List every alias set on this machine.
fn list(ctx: &Context) -> Result<()> {
    if ctx.config.aliases.is_empty() {
        say("No aliases set. Add one: byteferret alias <device-id> <alias>");
    } else {
        say("aliases:");
        for (id, a) in &ctx.config.aliases {
            say(&format!("  - {} → {id}", sanitize(a)));
        }
    }
    emit(&json!({ "ok": true, "aliases": ctx.config.aliases }));
    Ok(())
}

/// Resolve the device argument to a full device id. An existing alias or an
/// unambiguous prefix of a known/pending device is resolved via the agent; a
/// complete device id is accepted as-is even for a machine never paired with,
/// so it can be labelled ahead of time.
fn resolve(ctx: &Context, target: &str) -> Result<String> {
    // An existing alias simply re-targets the same device.
    if let Some(id) = ctx.config.device_for_alias(target) {
        return Ok(id.to_string());
    }

    let looks_full = normalize_id(target).len() >= MIN_FULL_ID_LEN;

    // Fall back to the raw id when the agent isn't reachable — labelling doesn't
    // actually need Syncthing, only prefix resolution does.
    let Ok(devices) = ctx.client.get_devices() else {
        if looks_full {
            return Ok(target.to_string());
        }
        bail!(
            "cannot reach the agent to look up '{target}' — start it with \
             `byteferret start`, or pass the full device id."
        );
    };

    let mut candidates: Vec<(String, String)> = devices
        .iter()
        .filter_map(|d| d.get("deviceID").and_then(|v| v.as_str()).map(|id| {
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            (id.to_string(), name)
        }))
        .collect();
    if let Ok(pending) = ctx.client.pending_devices() {
        for (id, info) in pending {
            if !candidates.iter().any(|(c, _)| *c == id) {
                candidates.push((id, info.name));
            }
        }
    }

    match resolve_device_target(target, &candidates) {
        Ok(id) => Ok(id),
        // A full device id that matches nothing known is a new machine being
        // labelled ahead of pairing — accept it rather than demanding it be known.
        Err(_) if looks_full => Ok(target.to_string()),
        Err(e) => Err(e),
    }
}

fn short(id: &str) -> String {
    id.chars().take(7).collect()
}
