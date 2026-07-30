//! `byteferret init <path> [--label <name>]` — bring a directory into sync.
//!
//! The first init on a machine becomes the *primary vault* and uses the
//! well-known folder id (`byteferret-vault`) so a fleet's default vaults link
//! up when paired. Every later init registers the directory as an additional
//! folder under an auto-generated id — the id is an implementation detail the
//! user never has to choose. Re-running init on an already-registered path
//! updates that folder in place (preserving its id and peers), so init is
//! always safe to repeat.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::agent::{ensure_started, vault_folder_config, Context};
use crate::fsutil::{expand_path, safe_dir_name};
use crate::output::{emit, sanitize, say};

const STIGNORE: &str = "\
// ByteFerret-managed ignore list
.stversions
.stfolder
*~
.*.swp
.DS_Store
";

/// Write a file only if it does not already exist (never clobber user content).
fn write_if_absent(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn scaffold(root: &Path) -> Result<()> {
    write_if_absent(
        &root.join("README.md"),
        "# My ByteFerret Vault\n\nEdit any doc in neovim; it syncs to your other machines automatically.\nCross-reference docs anywhere with `[[wiki-links]]`.\n",
    )?;
    Ok(())
}

/// Reduce a human name (label or directory name) to a folder-id slug.
fn folder_id_slug(name: &str) -> String {
    safe_dir_name(name)
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "folder".to_string())
}

/// A short random suffix so independently created folders never collide on id:
/// two machines both initing `~/recipes` must stay two distinct folders until
/// one is deliberately shared and accepted on the other.
fn random_suffix() -> Result<String> {
    let mut buf = [0u8; 3];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

/// A fresh folder id derived from the folder's name, unique on this machine.
fn generate_folder_id(ctx: &Context, name: &str) -> Result<String> {
    let slug = folder_id_slug(name);
    loop {
        let id = format!("{slug}-{}", random_suffix()?);
        if ctx.client.get_folder(&id)?.is_none() {
            return Ok(id);
        }
    }
}

/// Device ids a registered folder is already shared with.
fn folder_peers(folder: &Value) -> Vec<String> {
    folder
        .get("devices")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| d.get("deviceID").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn init(target: &str, existing: bool, label: Option<&str>) -> Result<()> {
    let path = expand_path(target);
    if existing && !path.exists() {
        bail!("--existing given but {} does not exist", path.display());
    }

    let ctx = ensure_started()?;

    fs::create_dir_all(&path)?;
    let abs = fs::canonicalize(&path)?;
    let path_str = abs.to_string_lossy().to_string();
    let dir_name = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "folder".to_string());

    // Re-init detection is by *path*, so running init twice on the same
    // directory updates the existing registration instead of duplicating it.
    let registered = ctx.client.get_folders()?.into_iter().find(|f| {
        f.get("path")
            .and_then(Value::as_str)
            .map(|p| fs::canonicalize(p).map(|c| c == abs).unwrap_or(p == path_str))
            .unwrap_or(false)
    });

    // First init (or re-init of the vault) is the primary vault; anything else
    // is an additional folder with its own generated id.
    let (folder_id, folder_label, peers, is_primary) = if let Some(f) = &registered {
        let id = f.get("id").and_then(Value::as_str).unwrap_or(&ctx.config.folder_id).to_string();
        let old_label = f.get("label").and_then(Value::as_str).unwrap_or("").to_string();
        let keep = if old_label.is_empty() { dir_name.clone() } else { old_label };
        let is_primary = id == ctx.config.folder_id;
        (id, label.map(str::to_string).unwrap_or(keep), folder_peers(f), is_primary)
    } else if ctx.config.vault_path.as_deref().map(|v| v == path_str).unwrap_or(true) {
        // Preserve any peers already granted to the vault id (e.g. the vault
        // directory moved and is being re-initialized at its new home).
        let peers = ctx.client.get_folder(&ctx.config.folder_id)?.as_ref().map(folder_peers).unwrap_or_default();
        (ctx.config.folder_id.clone(), label.unwrap_or("ByteFerret Vault").to_string(), peers, true)
    } else {
        let name = label.unwrap_or(&dir_name);
        (generate_folder_id(&ctx, name)?, name.to_string(), vec![], false)
    };

    // Starter content only for a brand-new primary vault; an existing directory
    // (or an additional folder) is taken as-is — init must never plant files in
    // a directory the user already owns the contents of.
    if is_primary && !existing && registered.is_none() {
        scaffold(&abs)?;
    }
    write_if_absent(&abs.join(".stignore"), STIGNORE)?;

    let folder = vault_folder_config(&folder_id, &folder_label, &path_str, &peers);
    ctx.client.put_folder(&folder)?;

    let mut config = ctx.config.clone();
    if is_primary {
        config.vault_path = Some(path_str.clone());
        config.save(&ctx.paths)?;
    }

    if is_primary {
        say(&format!("Vault ready at {path_str}"));
        say(&format!("  folder id: {folder_id}"));
        say(if registered.is_some() || existing {
            "  attached existing folder (no files changed)"
        } else {
            "  scaffolded starter structure"
        });
        say("");
        say("Next: get this machine's device id with `byteferret status`, then run");
        say("`byteferret pair --with <id>` on the other machine.");
    } else {
        say(&format!("Folder ready at {path_str}"));
        say(&format!("  folder id: {}  (\"{}\")", sanitize(&folder_id), sanitize(&folder_label)));
        say(if registered.is_some() {
            "  updated existing registration (id and peers unchanged)"
        } else {
            "  registered (no files changed)"
        });
        say("");
        say("Share it with a paired machine:");
        say(&format!("  byteferret pair --with <device-id> --folder {}", sanitize(&folder_id)));
    }
    emit(&json!({
        "ok": true,
        "path": path_str,
        "folderId": folder_id,
        "label": folder_label,
        "primary": is_primary,
        "vaultPath": config.vault_path,
        "updated": registered.is_some(),
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::folder_id_slug;

    #[test]
    fn slugs_are_lowercase_and_safe() {
        assert_eq!(folder_id_slug("Recipes"), "recipes");
        assert_eq!(folder_id_slug("Work Notes"), "work-notes");
        // Peer-influenced or odd names reduce to something safe, never empty.
        assert_eq!(folder_id_slug("../../etc"), "etc");
        assert_eq!(folder_id_slug("///"), "folder");
    }
}
