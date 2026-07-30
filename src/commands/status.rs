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

/// The folders currently connected with a peer, each annotated with its share
/// state (e.g. `byteferret-vault (establishing)`). Empty when the peer is
/// offline — nothing is connected — which the caller renders as "no folders
/// connected".
fn connected_folders(folder: &str, p: &PeerSync) -> Vec<String> {
    if !p.connected {
        return vec![];
    }
    let annotation = match p.share_state() {
        ShareState::Sharing => {
            if p.completion >= 99.5 {
                String::new()
            } else {
                format!(" ({:.0}%)", p.completion)
            }
        }
        ShareState::Establishing => " (establishing)".to_string(),
        ShareState::NotSharingBack => " (peer not sharing back)".to_string(),
        ShareState::Offline => String::new(), // unreachable: gated by `p.connected` above
    };
    vec![format!("{}{annotation}", sanitize(folder))]
}

/// Machine-readable form of the same list for `--json`: the folder id plus its
/// state, so consumers don't have to parse the annotated string.
fn folders_synced(folder: &str, p: &PeerSync) -> Value {
    if !p.connected {
        return json!([]);
    }
    json!([{ "folder": folder, "state": p.share_state().tag(), "completion": p.completion }])
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
    if peers.is_empty() {
        say("peers:      none — share the device id above and run `byteferret pair --with <id>` on the other machine");
    } else {
        say("peers:");
        for p in &peers {
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
            let folders = connected_folders(folder, p);
            if folders.is_empty() {
                say("      no folders connected");
            } else {
                for f in &folders {
                    say(&format!("      {f}"));
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
        "peers": peers.iter().map(|p| json!({
            "deviceId": p.id,
            "name": p.name,
            "connected": p.connected,
            "address": conns.get(&p.id).map(|c| c.address.clone()).unwrap_or_default(),
            "remoteState": p.remote_state,
            "sharingVault": p.sharing(),
            "shareState": p.share_state().tag(),
            "foldersSynced": folders_synced(folder, p),
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
