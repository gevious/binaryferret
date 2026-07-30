use anyhow::Result;
use serde_json::json;

use crate::agent::ensure_started;
use crate::output::{emit, say};

/// Start (or confirm) the agent's managed Syncthing. Idempotent.
pub fn start() -> Result<()> {
    let ctx = ensure_started()?;
    let id = ctx.client.my_device_id()?;
    let version = ctx.client.version()?;
    ctx.config.save(&ctx.paths)?;

    say("ByteFerret agent running.");
    say(&format!("  syncthing:  {version}  ({})", ctx.config.gui_address));
    say(&format!("  device id:  {id}"));
    match &ctx.config.vault_path {
        Some(v) => say(&format!("  vault:      {v}")),
        None => say("  vault:      (none yet — run `byteferret init <path>`)"),
    }
    emit(&json!({
        "ok": true, "running": true, "deviceId": id,
        "syncthingVersion": version, "guiAddress": ctx.config.gui_address,
    }));
    Ok(())
}
