//! Direct peer-to-peer pairing (Path A). No hub, no auto-accept.
//!
//! Pairing is two separate decisions, and this command keeps them separate:
//!
//!  1. **the connection** — `pair <device-id> --accept` admits a machine. On its
//!     own that grants access to nothing.
//!  2. **each folder** — `pair <device-id> --accept --folder <name>` shares one
//!     folder with that machine, or takes up one it offered. Repeat the flag to
//!     act on several folders from the same peer; `--reject --folder <name>`
//!     declines or withdraws exactly one.
//!
//! Both verbs always name one peer explicitly, by device id (or a local alias).
//! There is no "accept everything waiting" — on a shared network anyone can put a
//! request in your pending list, and a bulk accept would hand them a folder
//! alongside the machine you actually meant to approve.
//!
//! Typical flow:
//!   desktop 1:  byteferret status                                 → prints its device ID
//!   desktop 2:  byteferret pair --with <id-1> --folder <name>     → adds + shares a folder
//!   desktop 1:  byteferret pair <id-2> --accept                   → approves the connection
//!   desktop 1:  byteferret pair <id-2> --accept --folder <name>   → shares/takes up a folder

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::json;

use crate::agent::{
    add_device, create_shared_folder, folder_name, folder_registered_at, resolve_folder,
    resolve_peer, share_folder, unshare_folder, Context,
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
    // The bare positional argument identifies the peer for `--accept`/`--reject`
    // and has no meaning alongside `--with`. Catch the easy mistake of typing a
    // folder there (`pair --with <peer> <folder>`) before Syncthing acts on it:
    // left unchecked, clap parses the folder as the positional and it is silently
    // ignored, so the wrong set of folders gets shared.
    if let (Some(peer), Some(stray)) = (a.with, a.target) {
        bail!(
            "`pair --with <peer>` takes only the peer to pair with — '{}' was not \
             understood.\nTo share a specific folder with the peer, name it with --folder:\n  \
             byteferret pair --with {} --folder {}\n\
             (folder names come from `byteferret init` / `byteferret status`)",
            sanitize(stray),
            sanitize(peer),
            sanitize(stray),
        );
    }

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
    // `--with` normally takes the raw device id printed by the other machine, but
    // a previously-set alias for it is accepted too (aliases are trusted, being
    // locally set). An unknown id is passed through untouched so a brand-new peer
    // can still be added the first time.
    let peer_owned = ctx.config.device_for_alias(peer).map(str::to_string);
    let peer = peer_owned.as_deref().unwrap_or(peer);
    // Initiating the pairing is itself the decision to share. There is no
    // privileged default folder, so name the folder(s) with --folder; when the
    // machine has exactly one folder we take that as the obvious intent.
    let folders: Vec<String> = if a.folders.is_empty() {
        let ids = local_folder_ids(client)?;
        match ids.as_slice() {
            [] => bail!(
                "no folders here yet — run `byteferret init <path>` first, then \
                 `byteferret pair --with {} --folder <name>`.",
                short(peer)
            ),
            [one] => vec![one.clone()],
            _ => bail!(
                "this machine has several folders — name which to share with --folder <name>:{}",
                folder_name_list(&ids)
            ),
        }
    } else {
        a.folders.iter().map(|f| resolve_folder(client, f)).collect::<Result<Vec<_>>>()?
    };

    add_device(client, peer, a.name, a.address)?;
    for fid in &folders {
        share_folder(client, fid, peer)?;
    }
    say(&format!(
        "Added {}… and shared {} with it.",
        short(peer),
        join_folders(&folder_names(&folders))
    ));

    let connected = client
        .connections()
        .ok()
        .and_then(|c| c.get(peer).map(|c| c.connected))
        .unwrap_or(false);
    say("");
    if connected {
        say("That peer is already connected, so the folder is now shared both ways —");
        say("syncing will begin shortly. Confirm with `byteferret status`. (No `--accept` needed.)");
    } else {
        say("Not connected to that peer yet. If they already know each other, they'll");
        say("link up and sync on their own once both are online — nothing more to do.");
        say("If they don't connect within a minute, the other machine may still need to");
        say("accept this device. Check there with `byteferret status`.");
    }
    emit(&json!({
        "ok": true, "action": "with", "peerId": peer,
        "folders": folder_names(&folders), "folderIds": folders, "alreadyConnected": connected,
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
    let device_id = resolve_peer(&ctx.config, target, &cand_list)?;
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
        // The user names folders by their visible name; resolve each to a real
        // id, looking at both our own folders and the ones this peer offers.
        a.folders
            .iter()
            .map(|f| resolve_folder_ref(ctx, f, &offered))
            .collect()
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
        affected.push(folder_name(fid).to_string());
    }

    emit(&json!({
        "ok": true, "action": action, "peerId": device_id, "folders": affected,
    }));
    Ok(())
}

/// Map a user-typed folder reference to a real Syncthing id when acting on a
/// peer: a folder of ours (by name/id) or one the peer is offering (by name/id).
/// Falls through to the raw input so the downstream "not offered / not here"
/// message is what the user sees for a genuine typo.
fn resolve_folder_ref(ctx: &Context, input: &str, offered: &BTreeMap<String, String>) -> String {
    if let Ok(id) = resolve_folder(&ctx.client, input) {
        return id;
    }
    offered
        .keys()
        .find(|oid| folder_name(oid).eq_ignore_ascii_case(input) || oid.as_str() == input)
        .cloned()
        .unwrap_or_else(|| input.to_string())
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
        say(&format!("Already shared with it: {}.", join_folders(&folder_names(&shared))));
    }
    if !offered.is_empty() {
        say("It is offering these folders — accept the ones you want:");
        for (fid, label) in offered {
            let label = sanitize(label);
            let named = if label.is_empty() { String::new() } else { format!(" \"{label}\"") };
            say(&format!("  {}{named}", sanitize(folder_name(fid))));
            say(&format!(
                "    byteferret pair {} --accept --folder {}",
                short(device_id),
                sanitize(folder_name(fid))
            ));
        }
    }
    if shared.is_empty() && offered.is_empty() {
        // No default folder exists any more; point at a real one if there is any.
        say("No folders are shared with it yet, and it isn't offering any. To share one of yours:");
        let example = local_folder_ids(client)
            .ok()
            .and_then(|ids| ids.first().map(|id| folder_name(id).to_string()))
            .unwrap_or_else(|| "<name>".to_string());
        say(&format!(
            "  byteferret pair {} --accept --folder {}",
            short(device_id),
            example
        ));
    }
    emit(&json!({
        "ok": true, "action": "accept", "peerId": device_id, "folders": [],
        "offeredFolders": offered.keys().map(|id| folder_name(id)).collect::<Vec<_>>(),
        "sharedFolders": folder_names(&shared),
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
            format!("shared '{}' with {}", sanitize(folder_name(folder_id)), short(device_id))
        } else {
            format!("'{}' was already shared with {}", sanitize(folder_name(folder_id)), short(device_id))
        });
    }

    let Some(label) = offered.get(folder_id) else {
        bail!(
            "{} is not offering a folder '{}', and you don't have one by that name.\n\
             Run `byteferret status` to see what it offers.",
            short(device_id),
            sanitize(folder_name(folder_id))
        );
    };

    let dir = match a.path {
        Some(p) => {
            let dir = expand_path(p);
            // An explicit path is honoured, but never onto another folder's
            // directory — that is the one thing we must not do silently.
            if let Some(other) = folder_registered_at(client, &dir)? {
                bail!(
                    "{} is already the folder '{}' on this machine — accepting '{}' there would \
                     merge the two and overwrite files. Pick a different --path.",
                    dir.display(),
                    sanitize(&other),
                    sanitize(folder_id)
                );
            }
            dir
        }
        None => default_folder_path(ctx, folder_id, label)?,
    };
    create_shared_folder(client, folder_id, label, &dir.to_string_lossy(), device_id)?;
    Ok(format!(
        "accepted '{}' from {} → {}",
        sanitize(folder_name(folder_id)),
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
            sanitize(folder_name(folder_id)),
            short(device_id)
        ));
    }
    if unshare_folder(client, folder_id, device_id)? {
        return Ok(format!(
            "stopped sharing '{}' with {} (the folder and its files stay here)",
            sanitize(folder_name(folder_id)),
            short(device_id)
        ));
    }
    Ok(format!(
        "nothing to do for '{}' — {} neither offers it nor has access to it",
        sanitize(folder_name(folder_id)),
        short(device_id)
    ))
}

/// Where a folder taken up from a peer should live when `--path` is not given:
/// a *new* directory in the current working directory, named after the peer's
/// label — the same "init a folder wherever you are" model as `byteferret init`.
///
/// The chosen directory is always fresh: never a directory another folder already
/// owns (which a send-receive folder would start merging into and overwrite), and
/// never one that already has contents. When the obvious name is taken, we fall
/// back to the folder's globally-unique id so a distinct new folder is still
/// created automatically rather than clobbering an existing one.
fn default_folder_path(ctx: &Context, folder_id: &str, label: &str) -> Result<PathBuf> {
    let base = std::env::current_dir().map_err(|e| {
        anyhow::anyhow!(
            "cannot determine the current directory ({e}) — pass --path <dir> to choose where '{}' goes",
            sanitize(folder_id)
        )
    })?;

    // The label and the id both come from the peer, so neither is used as a path
    // until it has been reduced to a single safe component.
    let Some(name) = safe_dir_name(label).or_else(|| safe_dir_name(folder_id)) else {
        bail!(
            "the folder's name from the peer has no usable characters — \
             pass --path <dir> to choose where it goes."
        );
    };

    let preferred = base.join(&name);
    if is_free_for_new_folder(&ctx.client, &preferred)? {
        return Ok(preferred);
    }

    // The obvious name is already an existing folder or a non-empty directory.
    // Disambiguate with the peer's unique folder id so we still create a new,
    // separate folder instead of overwriting whatever is there.
    if let Some(id_name) = safe_dir_name(folder_id) {
        let alt = base.join(&id_name);
        if id_name != name && is_free_for_new_folder(&ctx.client, &alt)? {
            return Ok(alt);
        }
    }

    bail!(
        "{} already exists (another folder or non-empty directory) — pass --path <dir> to \
         choose where '{}' goes.",
        preferred.display(),
        sanitize(folder_id)
    );
}

/// A directory we may safely create a brand-new folder in: not already registered
/// to another folder here, and empty if it exists on disk at all.
fn is_free_for_new_folder(client: &Client, dir: &Path) -> Result<bool> {
    if folder_registered_at(client, dir)?.is_some() {
        return Ok(false);
    }
    let non_empty = dir.read_dir().map(|mut e| e.next().is_some()).unwrap_or(false);
    Ok(!non_empty)
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

/// Every folder id registered on this machine.
fn local_folder_ids(client: &Client) -> Result<Vec<String>> {
    Ok(client
        .get_folders()?
        .iter()
        .filter_map(|f| f.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect())
}

/// The visible names of a set of folder ids (suffix stripped).
fn folder_names(ids: &[String]) -> Vec<String> {
    ids.iter().map(|id| folder_name(id).to_string()).collect()
}

/// A bulleted list of folder names for an error hint.
fn folder_name_list(ids: &[String]) -> String {
    ids.iter().map(|id| format!("\n  {}", folder_name(id))).collect()
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
