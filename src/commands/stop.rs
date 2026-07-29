use anyhow::Result;
use serde_json::json;

use crate::output::{emit, say};
use crate::paths::Paths;
use crate::syncthing::process;

/// Stop the managed Syncthing. Idempotent.
pub fn stop() -> Result<()> {
    let res = process::stop(&Paths::resolve())?;
    say(if res.was_running { "BinaryFerret agent stopped." } else { "BinaryFerret agent was not running." });
    emit(&json!({ "ok": true, "wasRunning": res.was_running }));
    Ok(())
}
