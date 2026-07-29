use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::agent::load_context;
use crate::fsutil::find_files;
use crate::output::{emit, say};
use crate::syncthing::process;

/// Reports agent + Syncthing health, the vault, connected peers, sync state,
/// pending pair requests, and any conflict files (FR-16, FR-29). Read-only:
/// never starts Syncthing — if it's down, that's the headline.
pub fn status() -> Result<()> {
    let ctx = load_context()?;
    let key = if ctx.api_key.is_empty() { None } else { Some(ctx.api_key.as_str()) };
    let running = process::is_running(&ctx.paths, &ctx.config.gui_address, key);

    if !running {
        say("agent:      stopped   (run `binaryferret start`)");
        if let Some(v) = &ctx.config.vault_path {
            say(&format!("vault:      {v}"));
        }
        emit(&json!({ "ok": true, "agentRunning": false, "vaultPath": ctx.config.vault_path }));
        return Ok(());
    }

    let device_id = ctx.client.my_device_id()?;
    let version = ctx.client.version()?;
    let devices = ctx.client.get_devices()?;
    let conns = ctx.client.connections()?;
    let pending = ctx.client.pending_devices()?;

    let mut peers: Vec<Value> = Vec::new();
    for d in &devices {
        let did = d.get("deviceID").and_then(Value::as_str).unwrap_or("");
        if did == device_id || did.is_empty() {
            continue;
        }
        let name = d.get("name").and_then(Value::as_str).unwrap_or("");
        let conn = conns.get(did);
        peers.push(json!({
            "deviceId": did,
            "name": name,
            "connected": conn.map(|c| c.connected).unwrap_or(false),
            "address": conn.map(|c| c.address.clone()).unwrap_or_default(),
        }));
    }

    let mut folder_state = String::new();
    let mut need_bytes: i64 = 0;
    if ctx.config.vault_path.is_some() {
        if let Ok(st) = ctx.client.folder_status(&ctx.config.folder_id) {
            folder_state = st.state;
            need_bytes = st.need_bytes;
        }
    }

    let conflicts: Vec<String> = match &ctx.config.vault_path {
        Some(v) => find_files(Path::new(v), ".sync-conflict-", 100).iter().map(|p| p.to_string_lossy().to_string()).collect(),
        None => vec![],
    };
    let pending_ids: Vec<String> = pending.keys().cloned().collect();

    // --- human output ---
    say("agent:      running");
    say(&format!("syncthing:  {version}  ({})", ctx.config.gui_address));
    say(&format!("device id:  {device_id}"));
    match &ctx.config.vault_path {
        Some(v) => say(&format!("vault:      {v}")),
        None => say("vault:      (none — run `binaryferret init`)"),
    }
    if ctx.config.vault_path.is_some() {
        let extra = if need_bytes > 0 { format!("  ({need_bytes} bytes to sync)") } else { String::new() };
        say(&format!("sync state: {}{extra}", if folder_state.is_empty() { "unknown" } else { &folder_state }));
    }
    say("mode:       p2p");
    if peers.is_empty() {
        say("peers:      none (run `binaryferret pair --show`)");
    } else {
        say("peers:");
        for p in &peers {
            let did = p["deviceId"].as_str().unwrap_or("");
            let connected = p["connected"].as_bool().unwrap_or(false);
            let addr = p["address"].as_str().unwrap_or("");
            let addr = if addr.is_empty() { String::new() } else { format!("  {addr}") };
            say(&format!(
                "  - {} ({}…): {}{addr}",
                p["name"].as_str().unwrap_or(""),
                &did.chars().take(7).collect::<String>(),
                if connected { "connected" } else { "disconnected" },
            ));
        }
    }
    if !pending_ids.is_empty() {
        say(&format!("pending:    {} device request(s) — run `binaryferret pair --accept`", pending_ids.len()));
        for id in &pending_ids {
            let nm = pending.get(id).map(|p| p.name.as_str()).unwrap_or("");
            say(&format!("  - {nm} ({}…)", id.chars().take(7).collect::<String>()));
        }
    }
    if !conflicts.is_empty() {
        say(&format!("conflicts:  {} sync-conflict file(s) — open both copies, merge, delete the conflict copy:", conflicts.len()));
        for f in &conflicts {
            say(&format!("  - {f}"));
        }
    }

    emit(&json!({
        "ok": true,
        "agentRunning": true,
        "deviceId": device_id,
        "syncthingVersion": version,
        "guiAddress": ctx.config.gui_address,
        "vaultPath": ctx.config.vault_path,
        "folderId": ctx.config.folder_id,
        "syncState": if folder_state.is_empty() { Value::Null } else { Value::String(folder_state) },
        "needBytes": need_bytes,
        "mode": "p2p",
        "peers": peers,
        "pendingDevices": pending_ids,
        "conflicts": conflicts,
    }));
    Ok(())
}
