use anyhow::Result;
use serde_json::json;

use crate::agent::load_context;
use crate::output::{emit, say};
use crate::paths::SYNCTHING_VERSION;
use crate::syncthing::process;

pub fn version() -> Result<()> {
    let ctx = load_context()?;
    let key = if ctx.api_key.is_empty() { None } else { Some(ctx.api_key.as_str()) };
    let running = process::is_running(&ctx.paths, &ctx.config.gui_address, key);
    let st_version = if running { ctx.client.version().ok() } else { None };

    say(&format!("byteferret {}", env!("CARGO_PKG_VERSION")));
    let running_note = st_version.as_ref().map(|v| format!("  (running: {v})")).unwrap_or_default();
    say(&format!("syncthing (pinned): v{SYNCTHING_VERSION}{running_note}"));
    emit(&json!({
        "ok": true,
        "byteferret": env!("CARGO_PKG_VERSION"),
        "syncthingPinned": format!("v{SYNCTHING_VERSION}"),
        "syncthingRunning": st_version,
    }));
    Ok(())
}
