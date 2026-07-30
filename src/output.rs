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

/// Make a remote-controlled string safe to print to a terminal.
///
/// Device names, folder labels and folder ids all arrive from a *peer*, which
/// means an unfriendly one can put ANSI escape sequences in them and repaint the
/// terminal — hiding its own entry from `pair --show`, or forging an extra line
/// that tells the user to accept it. Control characters are replaced (not
/// dropped) so the value still reads as suspicious rather than silently
/// shortening, and the result is length-capped so a very long name can't push
/// the surrounding output off screen.
///
/// `--json` output is left alone: serde escapes control characters itself, and
/// consumers there aren't a terminal.
pub fn sanitize(s: &str) -> String {
    const MAX: usize = 96;
    let mut out = String::with_capacity(s.len().min(MAX));
    for c in s.chars().take(MAX) {
        // C0 (incl. ESC), DEL, and the C1 range that some terminals also honour.
        if c.is_control() || ('\u{80}'..='\u{9f}').contains(&c) {
            out.push('\u{fffd}');
        } else {
            out.push(c);
        }
    }
    if s.chars().count() > MAX {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(sanitize("my-laptop"), "my-laptop");
        assert_eq!(sanitize("Büro-Rechner"), "Büro-Rechner");
    }

    #[test]
    fn escape_sequences_are_neutralized() {
        // A name that would otherwise clear the line and forge its own output.
        assert_eq!(sanitize("evil\u{1b}[2K\rtrusted"), "evil\u{fffd}[2K\u{fffd}trusted");
        assert!(!sanitize("a\u{1b}[31mb").contains('\u{1b}'));
        assert!(!sanitize("a\nb").contains('\n'));
        assert!(!sanitize("a\u{9b}b").contains('\u{9b}')); // C1 CSI
    }

    #[test]
    fn overlong_names_are_capped() {
        let out = sanitize(&"x".repeat(500));
        assert_eq!(out.chars().count(), 97); // 96 + the ellipsis
        assert!(out.ends_with('…'));
    }
}
