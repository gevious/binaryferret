use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{add_peer, ensure_started, peer_status, PeerSync, ShareState};
use crate::output::{emit, say};

/// Direct peer-to-peer pairing (Path A). No hub, no auto-accept:
///   desktop 1:  byteferret pair --show               → prints its device ID + peers
///   desktop 2:  byteferret pair --with <id-1>         → adds + shares with desktop 1
///   desktop 1:  byteferret pair <host> --accept       → approves desktop 2's request
///   desktop 1:  byteferret pair <host> --reject       → dismisses a request
/// After both sides know each other and share the folder id, Syncthing syncs.
pub fn pair(
    show: bool,
    with: Option<&str>,
    accept: bool,
    reject: bool,
    target: Option<&str>,
    address: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    let ctx = ensure_started()?;
    let client = &ctx.client;
    let folder = &ctx.config.folder_id;

    if show {
        let id = client.my_device_id()?;
        let peers = peer_status(&ctx).unwrap_or_default();
        let pending = client.pending_devices()?;

        say(&format!("Device ID: {id}"));
        say("Connected devices:");
        if peers.is_empty() {
            say("  (none paired yet — share the Device ID above and run `byteferret pair --with <id>` on the other machine)");
        } else {
            for p in &peers {
                let conn = if p.connected { "Connected" } else { "Not Connected" };
                say(&format!("  {}: {conn}", p.label()));
                let folders = connected_folders(folder, p);
                if folders.is_empty() {
                    say("    no folders connected");
                } else {
                    for f in &folders {
                        say(&format!("    {f}"));
                    }
                }
            }
        }

        // Pending requests: devices asking to pair *to* us. Printed per the
        // spec so it's obvious a request is waiting and how to act on it.
        if !pending.is_empty() {
            say("");
            for (pid, info) in &pending {
                let host = if info.name.is_empty() { short(pid) } else { info.name.clone() };
                say(&format!(
                    "{host} is waiting to accept pairing. type `byteferret pair {host} --accept|reject` to accept or reject it"
                ));
            }
        }

        emit(&json!({
            "ok": true,
            "deviceId": id,
            "awaitingAcceptance": pending.len(),
            "peers": peers.iter().map(|p| json!({
                "deviceId": p.id,
                "name": p.name,
                "connected": p.connected,
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
            // Not connected *right now* does not prove an accept is pending —
            // the machines may already know each other and simply be offline.
            // So point the user at `--show`, which reports the real state,
            // instead of asserting an accept step that may not be needed.
            say("Not connected to that peer yet. If they already know each other, they'll");
            say("link up and sync on their own once both are online — nothing more to do.");
            say("If they don't connect within a minute, the other machine may still need to");
            say("accept this device. Check there with `byteferret pair --show`.");
        }
        emit(&json!({ "ok": true, "action": "with", "peerId": peer, "alreadyConnected": connected }));
        return Ok(());
    }

    if accept || reject {
        if accept && reject {
            bail!("--accept and --reject cannot be used together.");
        }
        let action = if reject { "reject" } else { "accept" };
        let pending = client.pending_devices()?;

        // Resolve the target (a hostname or a device id / id-prefix) to the
        // matching pending device id(s). With no target, act on all pending.
        let matched: Vec<String> = match target {
            Some(t) => pending
                .iter()
                .filter(|(pid, info)| info.name == t || pid.as_str() == t || pid.starts_with(t))
                .map(|(pid, _)| pid.clone())
                .collect(),
            None => pending.keys().cloned().collect(),
        };

        if matched.is_empty() {
            match target {
                Some(t) => say(&format!("No pairing request from '{t}' is waiting.")),
                None => say("No pairing requests are waiting."),
            }
            // Distinguish "nothing has arrived yet" from "already paired" — the
            // latter is the confusing dead end where sync can still be broken
            // because a peer never shared the vault back.
            let peers = peer_status(&ctx).unwrap_or_default();
            if peers.is_empty() {
                say("(The other machine must run `byteferret pair --with <this-device-id>` first —");
                say(" get this device's id with `byteferret pair --show`.)");
            } else if peers.iter().any(|p| p.stalled()) {
                say("Note: a connected peer is NOT sharing the vault back — run `byteferret doctor` for the fix.");
            }
            emit(&json!({ "ok": true, "action": action, "affected": [] }));
            return Ok(());
        }

        let mut affected = Vec::new();
        for pid in &matched {
            let peer_name = pending.get(pid).map(|p| p.name.as_str()).filter(|s| !s.is_empty());
            let label = peer_name.map(str::to_string).unwrap_or_else(|| short(pid));
            if reject {
                client.dismiss_pending_device(pid)?;
                say(&format!("Rejected {label} ({}…).", short(pid)));
            } else {
                add_peer(client, folder, pid, peer_name, address)?;
                say(&format!("Accepted {label} ({}…) and shared the vault.", short(pid)));
            }
            affected.push(pid.clone());
        }
        emit(&json!({ "ok": true, "action": action, "affected": affected }));
        return Ok(());
    }

    bail!("usage: byteferret pair (--show | --with <device-id> | [<hostname-or-id>] --accept | [<hostname-or-id>] --reject) [--address <addr>] [--name <name>]");
}

/// The folders currently connected with a peer, each annotated with its share
/// state (e.g. `byteferret-vault (establishing)`). Empty when the peer is
/// offline — nothing is connected — which the caller renders as "no folders
/// connected". byteferret manages a single vault folder, so the list holds at
/// most one entry today, but the shape is a list so multi-folder is a no-op.
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
    vec![format!("{folder}{annotation}")]
}

/// Machine-readable form of the same list for `--json`: the folder id plus its
/// state, so consumers don't have to parse the annotated string. Empty when the
/// peer is offline, matching the human "no folders connected".
fn folders_synced(folder: &str, p: &PeerSync) -> serde_json::Value {
    if !p.connected {
        return json!([]);
    }
    json!([{ "folder": folder, "state": p.share_state().tag(), "completion": p.completion }])
}

fn short(id: &str) -> String {
    id.chars().take(7).collect()
}
