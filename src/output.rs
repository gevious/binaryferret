//! Human vs. machine output. Every command supports `--json`; when set, the
//! agent prints exactly one JSON object to stdout and nothing else, so the
//! neovim plugin and scripts can parse it (FR-24).

use std::sync::atomic::{AtomicBool, Ordering};

static JSON_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_json_mode(on: bool) {
    JSON_MODE.store(on, Ordering::Relaxed);
}

pub fn is_json_mode() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

/// Print a human-readable line (suppressed in --json mode).
pub fn say(line: &str) {
    if !is_json_mode() {
        println!("{line}");
    }
}

/// Emit the machine-readable result for a command (only in --json mode).
pub fn emit(value: &serde_json::Value) {
    if is_json_mode() {
        println!("{}", value);
    }
}
