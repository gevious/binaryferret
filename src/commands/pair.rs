use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{add_peer, ensure_started};
use crate::output::{emit, say};

/// Direct peer-to-peer pairing (Path A). No hub, no auto-accept:
///   desktop 1:  binaryferret pair --show          → prints its device ID
///   desktop 2:  binaryferret pair --with <id-1>    → adds + shares with desktop 1
///   desktop 1:  binaryferret pair --accept         → approves desktop 2's request
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
        say("This device's ID (share it with another machine, then run there:");
        say(&format!("  binaryferret pair --with {id}"));
        say(")");
        say("");
        say(&id);
        emit(&json!({ "ok": true, "deviceId": id }));
        return Ok(());
    }

    if let Some(peer) = with {
        add_peer(client, folder, peer, name, address)?;
        say(&format!("Added peer {}… and shared the vault with it.", short(peer)));
        say("On that machine, run `binaryferret pair --accept` to approve this device.");
        emit(&json!({ "ok": true, "action": "with", "peerId": peer }));
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
            say("(The other machine must run `binaryferret pair --with <this-device-id>` first.)");
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

    bail!("usage: binaryferret pair (--show | --with <device-id> | --accept [<device-id>]) [--address <addr>] [--name <name>]");
}

fn short(id: &str) -> String {
    id.chars().take(7).collect()
}
