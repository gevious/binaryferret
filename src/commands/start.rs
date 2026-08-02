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

    let folder_count = ctx.client.get_folders().map(|f| f.len()).unwrap_or(0);

    say("ByteFerret agent running.");
    say(&format!("  syncthing:  {version}  ({})", ctx.config.gui_address));
    say(&format!("  device id:  {id}"));
    if folder_count == 0 {
        say("  folders:    (none yet — run `byteferret init <path>`)");
    } else {
        say(&format!("  folders:    {folder_count}  (see `byteferret status`)"));
    }
    emit(&json!({
        "ok": true, "running": true, "deviceId": id,
        "syncthingVersion": version, "guiAddress": ctx.config.gui_address,
    }));
    Ok(())
}
