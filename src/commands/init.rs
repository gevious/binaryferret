use std::fs;
use std::path::Path;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::agent::{ensure_started, vault_folder_config};
use crate::fsutil::expand_path;
use crate::output::{emit, say};

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

pub fn init(target: &str, existing: bool) -> Result<()> {
    let path = expand_path(target);
    if existing && !path.exists() {
        bail!("--existing given but {} does not exist", path.display());
    }

    let ctx = ensure_started()?;

    fs::create_dir_all(&path)?;
    if !existing {
        scaffold(&path)?; // scaffold never overwrites (write_if_absent)
    }
    write_if_absent(&path.join(".stignore"), STIGNORE)?;

    let abs = fs::canonicalize(&path)?;
    let path_str = abs.to_string_lossy().to_string();

    // Register (or update) the vault as a Syncthing folder, preserving any peers
    // already shared with it so re-running init is safe (FR-4).
    let prior = ctx.client.get_folder(&ctx.config.folder_id)?;
    let peers: Vec<String> = prior
        .as_ref()
        .and_then(|f| f.get("devices"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|d| d.get("deviceID").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default();
    let folder = vault_folder_config(&ctx.config.folder_id, "ByteFerret Vault", &path_str, &peers);
    ctx.client.put_folder(&folder)?;

    let mut config = ctx.config.clone();
    config.vault_path = Some(path_str.clone());
    config.save(&ctx.paths)?;

    say(&format!("Vault ready at {path_str}"));
    say(&format!("  folder id: {}", config.folder_id));
    say(if existing { "  attached existing folder (no files changed)" } else { "  scaffolded starter structure" });
    say("");
    say("Next: pair another machine with `byteferret pair --show` here, then");
    say("`byteferret pair --with <id>` on the other machine.");
    emit(&json!({ "ok": true, "vaultPath": path_str, "folderId": config.folder_id, "scaffolded": !existing }));
    Ok(())
}
