//! Direct peer-to-peer pairing (Path A). No hub, no auto-accept.
//!
//! Pairing is two separate decisions, and this command keeps them separate:
//!
//!  1. **the connection** — `pair <device-id> --accept` admits a machine. On its
//!     own that grants access to nothing.
//!  2. **each folder** — `pair <device-id> --accept --folder <id>` shares one
//!     folder with that machine, or takes up one it offered. Repeat the flag to
//!     act on several folders from the same peer; `--reject --folder <id>`
//!     declines or withdraws exactly one.
//!
//! Both verbs always name one peer explicitly, by device id. There is no
//! "accept everything waiting" — on a shared network anyone can put a request in
//! your pending list, and a bulk accept would hand them the vault alongside the
//! machine you actually meant to approve.
//!
//! Typical flow:
//!   desktop 1:  byteferret status                              → prints its device ID
//!   desktop 2:  byteferret pair --with <id-1>                  → adds + shares the vault
//!   desktop 1:  byteferret pair <id-2> --accept                → approves the connection
//!   desktop 1:  byteferret pair <id-2> --accept --folder <f>   → shares/takes up a folder

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{
    add_device, create_shared_folder, resolve_device_target, share_folder, unshare_folder, Context,
};
use crate::fsutil::{expand_path, safe_dir_name};
use crate::output::{emit, sanitize, say};
use crate::syncthing::rest::Client;

/// Everything `byteferret pair` accepts, as parsed by clap.
pub struct PairArgs<'a> {
    pub with: Option<&'a str>,
    pub accept: bool,
    pub reject: bool,
    /// Device id (or an unambiguous prefix) the accept/reject applies to.
    pub target: Option<&'a str>,
    pub address: Option<&'a str>,
    pub name: Option<&'a str>,
    /// Folder ids to act on; repeatable.
    pub folders: Vec<String>,
    /// Act on every folder the peer currently offers.
    pub all_folders: bool,
    /// Where to put a folder taken up from a peer's offer.
    pub path: Option<&'a str>,
}

pub fn pair(a: PairArgs) -> Result<()> {
    let ctx = crate::agent::ensure_started()?;

    if let Some(peer) = a.with {
        return pair_with(&ctx, &a, peer);
    }
    if a.accept || a.reject {
        return accept_or_reject(&ctx, &a);
    }

    bail!(
        "usage:\n  \
         byteferret pair --with <device-id> [--folder <id>]…\n  \
         byteferret pair <device-id> --accept [--folder <id>… | --all-folders] [--path <dir>]\n  \
         byteferret pair <device-id> --reject [--folder <id>… | --all-folders]\n\
         (this machine's device id, peers, and pending requests: `byteferret status`)"
    );
}

// --- `--with` -------------------------------------------------------------

fn pair_with(ctx: &Context, a: &PairArgs, peer: &str) -> Result<()> {
    let client = &ctx.client;
    // Initiating the pairing is itself the decision to share, so `--with` shares
    // the configured vault unless the caller names other folders.
    let folders: Vec<String> = if a.folders.is_empty() {
        vec![ctx.config.folder_id.clone()]
    } else {
        a.folders.clone()
    };

    add_device(client, peer, a.name, a.address)?;
    for fid in &folders {
        if client.get_folder(fid)?.is_none() {
            say(&format!("(no folder '{}' on this machine — skipped; run `byteferret init` first)", sanitize(fid)));
            continue;
        }
        share_folder(client, fid, peer)?;
    }
    say(&format!(
        "Added {}… and shared {} with it.",
        short(peer),
        join_folders(&folders)
    ));

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
        say("Not connected to that peer yet. If they already know each other, they'll");
        say("link up and sync on their own once both are online — nothing more to do.");
        say("If they don't connect within a minute, the other machine may still need to");
        say("accept this device. Check there with `byteferret status`.");
    }
    emit(&json!({
        "ok": true, "action": "with", "peerId": peer,
        "folders": folders, "alreadyConnected": connected,
    }));
    Ok(())
}

// --- `--accept` / `--reject` ---------------------------------------------

fn accept_or_reject(ctx: &Context, a: &PairArgs) -> Result<()> {
    if a.accept && a.reject {
        bail!("--accept and --reject cannot be used together.");
    }
    let client = &ctx.client;
    let accepting = a.accept;
    let action = if accepting { "accept" } else { "reject" };

    let Some(target) = a.target else {
        bail!(
            "`--{action}` needs the device id of the peer to {action} — run `byteferret status` \
             to list them.\nThere is deliberately no way to {action} every waiting request at once: \
             anyone on your network can add themselves to that list."
        );
    };

    let pending_devices = client.pending_devices()?;
    let pending_folders = client.pending_folders()?;
    let my_id = client.my_device_id()?;

    // Candidates are the peers we could plausibly mean: those waiting to pair,
    // plus those already configured (whose folders we may still be deciding on).
    let mut candidates: BTreeMap<String, String> = BTreeMap::new();
    for (id, info) in &pending_devices {
        candidates.insert(id.clone(), info.name.clone());
    }
    for d in client.get_devices()? {
        let Some(id) = d.get("deviceID").and_then(|v| v.as_str()) else { continue };
        if id == my_id {
            continue;
        }
        let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        candidates.entry(id.to_string()).or_insert(name);
    }
    if candidates.is_empty() {
        bail!(
            "no peers are known or waiting. The other machine must run \
             `byteferret pair --with <this-device-id>` first — get this device's id with \
             `byteferret status`."
        );
    }

    let cand_list: Vec<(String, String)> = candidates.into_iter().collect();
    let device_id = resolve_device_target(target, &cand_list)?;
    let is_pending_device = pending_devices.contains_key(&device_id);

    // Which folders this peer is offering us right now.
    let offered: BTreeMap<String, String> = pending_folders
        .iter()
        .filter_map(|(fid, pf)| pf.offered_by.get(&device_id).map(|o| (fid.clone(), o.label.clone())))
        .collect();

    let folder_scope = a.all_folders || !a.folders.is_empty();
    if !folder_scope {
        return device_only(ctx, &device_id, accepting, is_pending_device, &offered, a);
    }

    let wanted: Vec<String> = if a.all_folders {
        let mut w: Vec<String> = offered.keys().cloned().collect();
        if !accepting {
            // Rejecting "all folders" also covers folders we currently share
            // with this peer — withdrawing access, not just declining offers.
            for f in shared_with(&ctx.client, &device_id)? {
                if !w.contains(&f) {
                    w.push(f);
                }
            }
        }
        if w.is_empty() {
            if accepting {
                bail!("{} is not offering any folders.", short(&device_id));
            }
            bail!(
                "{} is not offering any folders, and none are shared with it.",
                short(&device_id)
            );
        }
        w
    } else {
        a.folders.clone()
    };

    if accepting && a.path.is_some() && wanted.len() > 1 {
        bail!("--path applies to a single folder — name one --folder at a time when using it.");
    }

    let mut affected = Vec::new();
    for fid in &wanted {
        let outcome = if accepting {
            accept_folder(ctx, &device_id, fid, &offered, a, is_pending_device)?
        } else {
            reject_folder(ctx, &device_id, fid, &offered)?
        };
        say(&format!("  {outcome}"));
        affected.push(fid.clone());
    }

    emit(&json!({
        "ok": true, "action": action, "peerId": device_id, "folders": affected,
    }));
    Ok(())
}

/// `--accept` / `--reject` with no folder named: the *connection* decision.
fn device_only(
    ctx: &Context,
    device_id: &str,
    accepting: bool,
    is_pending_device: bool,
    offered: &BTreeMap<String, String>,
    a: &PairArgs,
) -> Result<()> {
    let client = &ctx.client;

    if !accepting {
        if !is_pending_device {
            bail!(
                "{} is already a configured peer, so there is no pairing request to reject.\n\
                 To stop sharing a folder with it: byteferret pair {} --reject --folder <id>",
                short(device_id),
                short(device_id)
            );
        }
        client.dismiss_pending_device(device_id)?;
        say(&format!("Rejected the pairing request from {device_id}."));
        emit(&json!({ "ok": true, "action": "reject", "peerId": device_id, "folders": [] }));
        return Ok(());
    }

    if is_pending_device {
        add_device(client, device_id, a.name, a.address)?;
        say(&format!("Accepted the connection from {device_id}."));
    } else {
        say(&format!("{} is already a configured peer.", short(device_id)));
    }

    // Deliberately no folder is shared here — admitting a machine and giving it
    // documents are separate decisions, so spell out how to make the second one.
    let shared = shared_with(client, device_id)?;
    say("");
    if !shared.is_empty() {
        say(&format!("Already shared with it: {}.", join_folders(&shared)));
    }
    if !offered.is_empty() {
        say("It is offering these folders — accept the ones you want:");
        for (fid, label) in offered {
            let label = sanitize(label);
            let named = if label.is_empty() { String::new() } else { format!(" \"{label}\"") };
            say(&format!("  {}{named}", sanitize(fid)));
            say(&format!(
                "    byteferret pair {} --accept --folder {}",
                short(device_id),
                sanitize(fid)
            ));
        }
    }
    if shared.is_empty() && offered.is_empty() {
        say("No folders are shared with it yet, and it isn't offering any. To share one of yours:");
        say(&format!(
            "  byteferret pair {} --accept --folder {}",
            short(device_id),
            ctx.config.folder_id
        ));
    }
    emit(&json!({
        "ok": true, "action": "accept", "peerId": device_id, "folders": [],
        "offeredFolders": offered.keys().collect::<Vec<_>>(),
        "sharedFolders": shared,
    }));
    Ok(())
}

/// Accept one folder from one peer: share ours back, or take up their offer.
fn accept_folder(
    ctx: &Context,
    device_id: &str,
    folder_id: &str,
    offered: &BTreeMap<String, String>,
    a: &PairArgs,
    is_pending_device: bool,
) -> Result<String> {
    let client = &ctx.client;
    // Sharing implies the connection, so admit the peer first if it was waiting.
    if is_pending_device {
        add_device(client, device_id, a.name, a.address)?;
    }

    if client.get_folder(folder_id)?.is_some() {
        let changed = share_folder(client, folder_id, device_id)?;
        return Ok(if changed {
            format!("shared '{}' with {}", sanitize(folder_id), short(device_id))
        } else {
            format!("'{}' was already shared with {}", sanitize(folder_id), short(device_id))
        });
    }

    let Some(label) = offered.get(folder_id) else {
        bail!(
            "{} is not offering a folder '{}', and you don't have one by that id.\n\
             Run `byteferret status` to see what it offers.",
            short(device_id),
            sanitize(folder_id)
        );
    };

    let dir = match a.path {
        Some(p) => expand_path(p),
        None => default_folder_path(ctx, folder_id, label)?,
    };
    create_shared_folder(client, folder_id, label, &dir.to_string_lossy(), device_id)?;
    Ok(format!(
        "accepted '{}' from {} → {}",
        sanitize(folder_id),
        short(device_id),
        dir.display()
    ))
}

/// Reject one folder from one peer: decline the offer, or withdraw access.
fn reject_folder(
    ctx: &Context,
    device_id: &str,
    folder_id: &str,
    offered: &BTreeMap<String, String>,
) -> Result<String> {
    let client = &ctx.client;
    if offered.contains_key(folder_id) {
        client.dismiss_pending_folder(folder_id, device_id)?;
        return Ok(format!(
            "declined '{}' from {}",
            sanitize(folder_id),
            short(device_id)
        ));
    }
    if unshare_folder(client, folder_id, device_id)? {
        return Ok(format!(
            "stopped sharing '{}' with {} (the folder and its files stay here)",
            sanitize(folder_id),
            short(device_id)
        ));
    }
    Ok(format!(
        "nothing to do for '{}' — {} neither offers it nor has access to it",
        sanitize(folder_id),
        short(device_id)
    ))
}

/// Where a folder taken up from a peer should live when `--path` is not given:
/// beside the existing vault, under a name derived from the peer's label.
///
/// Refuses rather than guesses in the two cases where guessing could do damage —
/// no vault to anchor to, or a directory already sitting at that name (which a
/// send-receive folder would start merging into).
fn default_folder_path(ctx: &Context, folder_id: &str, label: &str) -> Result<PathBuf> {
    let Some(vault) = &ctx.config.vault_path else {
        bail!(
            "no vault is configured, so there is nowhere obvious to put '{}' — \
             pass --path <dir> to choose.",
            sanitize(folder_id)
        );
    };
    let base = Path::new(vault).parent().unwrap_or(Path::new(vault)).to_path_buf();

    // The label and the id both come from the peer, so neither is used as a path
    // until it has been reduced to a single safe component.
    let Some(name) = safe_dir_name(label).or_else(|| safe_dir_name(folder_id)) else {
        bail!(
            "the folder's name from the peer has no usable characters — \
             pass --path <dir> to choose where it goes."
        );
    };
    let dir = base.join(&name);

    let occupied = dir.read_dir().map(|mut e| e.next().is_some()).unwrap_or(false);
    if occupied {
        bail!(
            "{} already exists and is not empty — pass --path <dir> to choose where '{}' goes, \
             or point it there deliberately.",
            dir.display(),
            sanitize(folder_id)
        );
    }
    Ok(dir)
}

/// Folder ids currently shared with `device_id`.
fn shared_with(client: &Client, device_id: &str) -> Result<Vec<String>> {
    Ok(client
        .get_folders()?
        .into_iter()
        .filter(|f| {
            f.get("devices")
                .and_then(|d| d.as_array())
                .map(|a| a.iter().any(|d| d.get("deviceID").and_then(|v| v.as_str()) == Some(device_id)))
                .unwrap_or(false)
        })
        .filter_map(|f| f.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect())
}

fn join_folders(folders: &[String]) -> String {
    match folders {
        [] => "no folders".to_string(),
        [one] => format!("'{}'", sanitize(one)),
        many => many.iter().map(|f| format!("'{}'", sanitize(f))).collect::<Vec<_>>().join(", "),
    }
}

fn short(id: &str) -> String {
    id.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::join_folders;

    #[test]
    fn folder_lists_read_naturally() {
        assert_eq!(join_folders(&["a".into()]), "'a'");
        assert_eq!(join_folders(&["a".into(), "b".into()]), "'a', 'b'");
    }

    #[test]
    fn folder_ids_from_a_peer_cannot_inject_escapes_into_the_summary() {
        let out = join_folders(&["ev\u{1b}[2Kil".to_string()]);
        assert!(!out.contains('\u{1b}'));
    }
}
