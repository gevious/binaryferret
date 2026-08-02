//! `byteferret unpair <folder> [--with <machine>]` — stop sharing a folder.
//!
//! Two scopes:
//!  - `--with <machine>` withdraws the folder from *one* peer (device id, prefix,
//!    or alias). The folder stays registered here and shared with any other
//!    peers — the same effect as `pair <peer> --reject --folder <name>`.
//!  - no `--with` removes the folder from this machine *entirely*: syncing stops
//!    and it is unshared from every peer at once. Because that is the broad,
//!    harder-to-undo action, it asks for confirmation first (skip with `--yes`).
//!
//! Either way the directory and its files on disk are left untouched — unpair
//! forgets the sharing, it never deletes documents. Re-add with `init <path>`.

use std::io::Write;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::agent::{ensure_started, folder_name, resolve_folder, resolve_peer, unshare_folder, Context};
use crate::output::{emit, is_json_mode, sanitize, say};

pub fn unpair(folder: &str, with: Option<&str>, yes: bool) -> Result<()> {
    let ctx = ensure_started()?;
    let id = resolve_folder(&ctx.client, folder)?;
    let name = folder_name(&id).to_string();

    match with {
        Some(peer) => unpair_from_peer(&ctx, &id, &name, peer),
        None => unpair_everywhere(&ctx, &id, &name, yes),
    }
}

/// Withdraw the folder from a single peer, leaving it registered and shared with
/// the rest.
fn unpair_from_peer(ctx: &Context, id: &str, name: &str, peer: &str) -> Result<()> {
    let device_id = resolve_peer(&ctx.config, peer, &configured_peers(ctx)?)?;
    let who = peer_label(ctx, &device_id);

    let changed = unshare_folder(&ctx.client, id, &device_id)?;
    if changed {
        say(&format!(
            "Unpaired '{}' from {who} — it stays here and shared with any other peers.",
            sanitize(name)
        ));
    } else {
        say(&format!("'{}' was not shared with {who} — nothing to do.", sanitize(name)));
    }
    emit(&json!({
        "ok": true, "action": "unpair", "scope": "peer",
        "name": name, "folderId": id, "peer": device_id, "changed": changed,
    }));
    Ok(())
}

/// Remove the folder from this machine entirely, after confirmation.
fn unpair_everywhere(ctx: &Context, id: &str, name: &str, yes: bool) -> Result<()> {
    let peers = share_peer_count(ctx, id)?;
    let path = ctx
        .client
        .get_folder(id)?
        .as_ref()
        .and_then(|f| f.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if !yes && !confirm_removal(name, peers)? {
        say("Aborted — nothing changed.");
        emit(&json!({ "ok": true, "action": "unpair", "scope": "all", "removed": false }));
        return Ok(());
    }

    ctx.client.delete_folder(id)?;
    say(&format!(
        "Unpaired '{}' — it no longer syncs and is unshared from all peers.",
        sanitize(name)
    ));
    if !path.is_empty() {
        say(&format!("  your files are untouched at {}", sanitize(&path)));
    }
    say("  re-add it any time with `byteferret init <path>`");
    emit(&json!({
        "ok": true, "action": "unpair", "scope": "all",
        "name": name, "folderId": id, "path": path, "removed": true,
    }));
    Ok(())
}

/// Ask before the broad removal. In `--json` mode there is no terminal to prompt
/// on, so refuse rather than guess — the caller must pass `--yes` deliberately.
fn confirm_removal(name: &str, peers: usize) -> Result<bool> {
    if is_json_mode() {
        bail!("refusing to remove '{name}' from this machine without --yes (there is no prompt in --json mode)");
    }
    let shared = match peers {
        0 => "It is not shared with any peers.".to_string(),
        1 => "It is shared with 1 peer.".to_string(),
        n => format!("It is shared with {n} peers."),
    };
    print!(
        "Remove '{}' from this machine? {shared} Syncing stops everywhere; your files stay on disk. [y/N] ",
        sanitize(name)
    );
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().chars().next(), Some('y') | Some('Y')))
}

/// Configured peers as `(id, name)` pairs (this device excluded), for resolving
/// a `--with` target by id/prefix/alias.
fn configured_peers(ctx: &Context) -> Result<Vec<(String, String)>> {
    let my_id = ctx.client.my_device_id()?;
    Ok(ctx
        .client
        .get_devices()?
        .iter()
        .filter_map(|d| {
            let id = d.get("deviceID").and_then(Value::as_str)?;
            if id == my_id {
                return None;
            }
            let name = d.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            Some((id.to_string(), name))
        })
        .collect())
}

/// How the peer reads in output: its local alias if set, else a short id prefix.
fn peer_label(ctx: &Context, device_id: &str) -> String {
    match ctx.config.alias_for(device_id) {
        Some(a) => sanitize(a),
        None => format!("{}…", device_id.chars().take(7).collect::<String>()),
    }
}

/// Number of peers (this device excluded) the folder is shared with.
fn share_peer_count(ctx: &Context, id: &str) -> Result<usize> {
    let my_id = ctx.client.my_device_id()?;
    Ok(ctx
        .client
        .get_folder(id)?
        .as_ref()
        .and_then(|f| f.get("devices"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| d.get("deviceID").and_then(Value::as_str))
                .filter(|d| *d != my_id)
                .count()
        })
        .unwrap_or(0))
}
