use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{add_peer, ensure_started, peer_status};
use crate::output::{emit, say};

/// Direct peer-to-peer pairing (Path A). No hub, no auto-accept:
///   desktop 1:  byteferret pair --show          → prints its device ID
///   desktop 2:  byteferret pair --with <id-1>    → adds + shares with desktop 1
///   desktop 1:  byteferret pair --accept         → approves desktop 2's request
/// After both sides know each other and share the folder id, Syncthing syncs.
pub fn pair(
    show: bool,
    with: Option<&str>,
    accept: bool,
    accept_id: Option<&str>,
    address: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    let ctx = ensure_started()?;
    let client = &ctx.client;
    let folder = &ctx.config.folder_id;

    if show {
        let id = client.my_device_id()?;
        say("This device's ID — share it with another machine, then run there:");
        say(&format!("  byteferret pair --with {id}"));
        say("");
        say(&id);
        say("");

        // Approved peers: devices already added here, with live connection state
        // and — the part that matters — whether they share the vault back.
        let peers = peer_status(&ctx).unwrap_or_default();
        if peers.is_empty() {
            say("Approved peers: none yet.");
        } else {
            say("Approved peers (already paired):");
            for p in &peers {
                let state = if !p.connected {
                    "offline".to_string()
                } else if p.sharing() {
                    format!("connected · sharing vault ✓ ({:.0}%)", p.completion)
                } else {
                    format!(
                        "connected · NOT sharing vault ✗ — run `byteferret pair --with {id}` there"
                    )
                };
                say(&format!("  - {} ({}…)  {state}", p.label(), short(&p.id)));
            }
        }
        say("");

        // Pending requests: devices that initiated pairing *to* us and are
        // waiting for approval (`pair --accept`).
        let pending = client.pending_devices()?;
        if pending.is_empty() {
            say("Pending requests: none.");
        } else {
            say("Pending requests (devices asking to pair — approve with `byteferret pair --accept`):");
            for (pid, info) in &pending {
                let nm = if info.name.is_empty() { String::new() } else { format!("{} ", info.name) };
                let from = if info.address.is_empty() { String::new() } else { format!(" from {}", info.address) };
                say(&format!("  - {nm}({}…){from}", short(pid)));
            }
        }

        emit(&json!({
            "ok": true,
            "deviceId": id,
            "peers": peers.iter().map(|p| json!({
                "deviceId": p.id,
                "name": p.name,
                "connected": p.connected,
                "remoteState": p.remote_state,
                "sharingVault": p.sharing(),
            })).collect::<Vec<_>>(),
            "pending": pending.iter().map(|(pid, info)| json!({
                "deviceId": pid,
                "name": info.name,
                "address": info.address,
            })).collect::<Vec<_>>(),
        }));
        return Ok(());
    }

    if let Some(peer) = with {
        add_peer(client, folder, peer, name, address)?;
        say(&format!("Added {}… and shared the vault with it.", short(peer)));
        // If the two machines are already connected as devices, the folder share
        // we just did was the missing half — no `--accept` step is needed.
        let connected = client
            .connections()
            .ok()
            .and_then(|c| c.get(peer).map(|c| c.connected))
            .unwrap_or(false);
        say("");
        if connected {
            say("That peer is already connected, so the vault is now shared both ways —");
            say("syncing will begin shortly. Confirm with `byteferret status`. (No `--accept` needed.)");
        } else {
            say("Next, on that machine approve this device:");
            say("  byteferret pair --accept");
        }
        emit(&json!({ "ok": true, "action": "with", "peerId": peer, "alreadyConnected": connected }));
        return Ok(());
    }

    if accept {
        let pending = client.pending_devices()?;
        let to_accept: Vec<String> = match accept_id {
            Some(x) => pending.keys().filter(|k| k.as_str() == x).cloned().collect(),
            None => pending.keys().cloned().collect(),
        };
        if to_accept.is_empty() {
            say(match accept_id {
                Some(x) => format!("No pending request from {x}."),
                None => "No pending device requests.".to_string(),
            }
            .as_str());
            // Distinguish "nothing has arrived yet" from "already paired" — the
            // latter is the confusing dead end where sync can still be broken
            // because a peer never shared the vault back.
            let peers = peer_status(&ctx).unwrap_or_default();
            if peers.is_empty() {
                say("(The other machine must run `byteferret pair --with <this-device-id>` first —");
                say(" get this device's id with `byteferret pair --show`.)");
            } else {
                say(&format!(
                    "You already have {} paired peer(s); nothing is waiting for approval.",
                    peers.len()
                ));
                if peers.iter().any(|p| p.stalled()) {
                    say("Note: a connected peer is NOT sharing the vault back — run `byteferret doctor` for the fix.");
                } else {
                    say("If files still aren't syncing, run `byteferret doctor`.");
                }
            }
            emit(&json!({ "ok": true, "action": "accept", "accepted": [] }));
            return Ok(());
        }
        for id in &to_accept {
            let peer_name = pending.get(id).map(|p| p.name.as_str()).filter(|s| !s.is_empty());
            add_peer(client, folder, id, peer_name, address)?;
            say(&format!("Accepted {} ({}…) and shared the vault.", peer_name.unwrap_or(""), short(id)));
        }
        emit(&json!({ "ok": true, "action": "accept", "accepted": to_accept }));
        return Ok(());
    }

    bail!("usage: byteferret pair (--show | --with <device-id> | --accept [<device-id>]) [--address <addr>] [--name <name>]");
}

fn short(id: &str) -> String {
    id.chars().take(7).collect()
}
