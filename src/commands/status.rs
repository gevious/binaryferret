use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::agent::{load_context, peer_status, PeerSync, ShareState};
use crate::fsutil::find_files;
use crate::output::{emit, sanitize, say};
use crate::syncthing::process;

/// This machine's hostname (via gethostname), or "" if it can't be read.
fn hostname() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    if !ok {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).to_string()
}

fn short(id: &str) -> String {
    id.chars().take(7).collect()
}

/// One registered Syncthing folder, as `status` reports it.
struct FolderInfo {
    id: String,
    label: String,
    path: String,
    /// Device ids the folder is shared with (this device excluded).
    peers: Vec<String>,
    state: String,
}

/// How one (folder, peer) pair renders next to the folder id, e.g.
/// ` (establishing)` or ` (42%)`; empty when fully in sync or offline.
fn share_annotation(state: ShareState, completion: f64) -> String {
    match state {
        ShareState::Sharing => {
            if completion >= 99.5 {
                String::new()
            } else {
                format!(" ({completion:.0}%)")
            }
        }
        ShareState::Establishing => " (establishing)".to_string(),
        ShareState::NotSharingBack => " (peer not sharing back)".to_string(),
        ShareState::Offline => String::new(),
    }
}

/// Per-folder share state between one peer and us: every folder shared with the
/// peer, with the same classification `doctor` uses for the vault. Ordered as
/// (folder id, state, completion).
fn peer_folder_states(
    ctx: &crate::agent::Context,
    folders: &[FolderInfo],
    p: &PeerSync,
) -> Vec<(String, ShareState, f64)> {
    folders
        .iter()
        .filter(|f| f.peers.contains(&p.id))
        .map(|f| {
            if !p.connected {
                return (f.id.clone(), ShareState::Offline, 0.0);
            }
            let (state, completion) = match ctx.client.folder_completion(&f.id, &p.id) {
                Ok(c) => (ShareState::classify(true, &c.remote_state), c.completion),
                Err(_) => (ShareState::Establishing, 0.0),
            };
            (f.id.clone(), state, completion)
        })
        .collect()
}

/// Reports agent + Syncthing health, this machine's identity (hostname, device
/// id), the vault, connected peers and their folders, sync state, pending pair
/// requests, offered folders, and any conflict files (FR-16, FR-29). This is
/// also where a machine's device id comes from during pairing. Read-only:
/// never starts Syncthing — if it's down, that's the headline.
pub fn status() -> Result<()> {
    let ctx = load_context()?;
    let host = hostname();
    let key = if ctx.api_key.is_empty() { None } else { Some(ctx.api_key.as_str()) };
    let running = process::is_running(&ctx.paths, &ctx.config.gui_address, key);

    if !running {
        say("agent:      stopped   (run `byteferret start`)");
        if !host.is_empty() {
            say(&format!("hostname:   {host}"));
        }
        if let Some(v) = &ctx.config.vault_path {
            say(&format!("vault:      {v}"));
        }
        emit(&json!({
            "ok": true, "agentRunning": false, "hostname": host,
            "vaultPath": ctx.config.vault_path,
        }));
        return Ok(());
    }

    let device_id = ctx.client.my_device_id()?;
    let version = ctx.client.version()?;
    let peers = peer_status(&ctx).unwrap_or_default();
    let conns = ctx.client.connections().unwrap_or_default();
    let pending = ctx.client.pending_devices()?;
    let pending_folders = ctx.client.pending_folders()?;
    let folder = &ctx.config.folder_id;

    // Every registered folder — the vault plus any additional `init`ed or
    // accepted folders — so multi-folder setups are fully visible here.
    let folders: Vec<FolderInfo> = ctx
        .client
        .get_folders()
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let id = f.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let state = ctx.client.folder_status(&id).map(|s| s.state).unwrap_or_default();
            FolderInfo {
                label: f.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
                path: f.get("path").and_then(Value::as_str).unwrap_or("").to_string(),
                peers: f
                    .get("devices")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|d| d.get("deviceID").and_then(Value::as_str))
                            .filter(|d| *d != device_id)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                id,
                state,
            }
        })
        .collect();

    let mut folder_state = String::new();
    let mut need_bytes: i64 = 0;
    if ctx.config.vault_path.is_some() {
        if let Ok(st) = ctx.client.folder_status(folder) {
            folder_state = st.state;
            need_bytes = st.need_bytes;
        }
    }

    let conflicts: Vec<String> = match &ctx.config.vault_path {
        Some(v) => find_files(Path::new(v), ".sync-conflict-", 100).iter().map(|p| p.to_string_lossy().to_string()).collect(),
        None => vec![],
    };

    // Per-peer folder share states, computed once and used by both the human
    // and the JSON rendering below.
    let shares: Vec<Vec<(String, ShareState, f64)>> =
        peers.iter().map(|p| peer_folder_states(&ctx, &folders, p)).collect();

    // --- human output ---
    say("agent:      running");
    say(&format!("syncthing:  {version}  ({})", ctx.config.gui_address));
    if !host.is_empty() {
        say(&format!("hostname:   {host}"));
    }
    say(&format!("device id:  {device_id}"));
    match &ctx.config.vault_path {
        Some(v) => say(&format!("vault:      {v}")),
        None => say("vault:      (none — run `byteferret init`)"),
    }
    if ctx.config.vault_path.is_some() {
        let extra = if need_bytes > 0 { format!("  ({need_bytes} bytes to sync)") } else { String::new() };
        say(&format!("sync state: {}{extra}", if folder_state.is_empty() { "unknown" } else { &folder_state }));
    }
    say("mode:       p2p");
    // The vault has its own lines above; list all folders once there is more
    // to the picture than the vault alone.
    if folders.iter().any(|f| f.id != *folder) {
        say("folders:");
        for f in &folders {
            let label = sanitize(&f.label);
            let named = if label.is_empty() { String::new() } else { format!(" (\"{label}\")") };
            let state = if f.state.is_empty() { "unknown".to_string() } else { f.state.clone() };
            say(&format!(
                "  - {}{named}  {}  — {state}, shared with {} peer(s)",
                sanitize(&f.id),
                sanitize(&f.path),
                f.peers.len(),
            ));
        }
    }
    if peers.is_empty() {
        say("peers:      none — share the device id above and run `byteferret pair --with <id>` on the other machine");
    } else {
        say("peers:");
        for (p, shared) in peers.iter().zip(&shares) {
            let addr = conns
                .get(&p.id)
                .map(|c| c.address.clone())
                .filter(|a| !a.is_empty())
                .map(|a| format!("  {}", sanitize(&a)))
                .unwrap_or_default();
            say(&format!(
                "  - {} ({}…): {}{addr}",
                sanitize(&p.label()),
                short(&p.id),
                if p.connected { "connected" } else { "disconnected" },
            ));
            if shared.is_empty() {
                say("      no folders shared");
            } else {
                for (fid, state, completion) in shared {
                    say(&format!(
                        "      {}{}",
                        sanitize(fid),
                        share_annotation(*state, *completion)
                    ));
                }
            }
        }
    }

    // The silent stall: connected, so the list above looks healthy, but the peer
    // never shared back, so nothing will ever transfer.
    if peers.iter().any(PeerSync::stalled) {
        say("");
        say("Note: a connected peer is NOT sharing a folder back — run `byteferret doctor` for the fix.");
    }

    // Requests to pair *with us*. The full device id is printed because that is
    // what `--accept` takes — and because the name beside it is the remote's
    // claim about itself, not something to act on.
    if !pending.is_empty() {
        say(&format!("pending:    {} pairing request(s):", pending.len()));
        for (pid, info) in &pending {
            let name = sanitize(&info.name);
            let named = if name.is_empty() { String::new() } else { format!(" ({name})") };
            say(&format!("  - {pid}{named}"));
            say(&format!("      accept:  byteferret pair {} --accept", short(pid)));
            say(&format!("      reject:  byteferret pair {} --reject", short(pid)));
        }
    }

    // Folders already-paired peers have offered us but that we have not taken up.
    if !pending_folders.is_empty() {
        say("offered folders:");
        for (fid, pf) in &pending_folders {
            for (did, offer) in &pf.offered_by {
                let label = sanitize(&offer.label);
                let named = if label.is_empty() { String::new() } else { format!(" \"{label}\"") };
                say(&format!("  - {}{named} — offered by {}", sanitize(fid), short(did)));
                say(&format!(
                    "      accept:  byteferret pair {} --accept --folder {}",
                    short(did),
                    sanitize(fid)
                ));
            }
        }
    }

    if !conflicts.is_empty() {
        say(&format!("conflicts:  {} sync-conflict file(s) — open both copies, merge, delete the conflict copy:", conflicts.len()));
        // Conflict *filenames* also arrive from a peer — they are whatever the
        // other machine named its copy — so they get the same treatment.
        for f in &conflicts {
            say(&format!("  - {}", sanitize(f)));
        }
    }

    emit(&json!({
        "ok": true,
        "agentRunning": true,
        "hostname": host,
        "deviceId": device_id,
        "syncthingVersion": version,
        "guiAddress": ctx.config.gui_address,
        "vaultPath": ctx.config.vault_path,
        "folderId": folder,
        "syncState": if folder_state.is_empty() { Value::Null } else { Value::String(folder_state) },
        "needBytes": need_bytes,
        "mode": "p2p",
        "folders": folders.iter().map(|f| json!({
            "id": f.id,
            "label": f.label,
            "path": f.path,
            "state": f.state,
            "peers": f.peers,
        })).collect::<Vec<_>>(),
        "peers": peers.iter().zip(&shares).map(|(p, shared)| json!({
            "deviceId": p.id,
            "name": p.name,
            "connected": p.connected,
            "address": conns.get(&p.id).map(|c| c.address.clone()).unwrap_or_default(),
            "remoteState": p.remote_state,
            "sharingVault": p.sharing(),
            "shareState": p.share_state().tag(),
            "foldersSynced": shared.iter().map(|(fid, state, completion)| json!({
                "folder": fid, "state": state.tag(), "completion": completion,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "pending": pending.iter().map(|(pid, info)| json!({
            "deviceId": pid,
            "name": info.name,
            "address": info.address,
        })).collect::<Vec<_>>(),
        "pendingFolders": pending_folders.iter().flat_map(|(fid, pf)| {
            pf.offered_by.iter().map(move |(did, offer)| json!({
                "folderId": fid,
                "offeredBy": did,
                "label": offer.label,
                "time": offer.time,
                "receiveEncrypted": offer.receive_encrypted,
                "remoteEncrypted": offer.remote_encrypted,
            }))
        }).collect::<Vec<_>>(),
        "conflicts": conflicts,
    }));
    Ok(())
}
