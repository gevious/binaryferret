//! User-service integration (FR-1): the unit/agent definitions ByteFerret
//! installs so the agent starts on login. On Linux this is a systemd
//! `Type=oneshot` **user** service with `RemainAfterExit=yes`:
//!   ExecStart = `byteferret start`  (brings the managed Syncthing up, detached)
//!   ExecStop  = `byteferret stop`   (takes it back down)
//! On macOS it is a launchd user agent with `RunAtLoad` running the same
//! `byteferret start` (launchd has no ExecStop; `service uninstall` stops the
//! managed Syncthing itself).
//!
//! We deliberately avoid a long-lived foreground supervisor: `start`/`stop` are
//! already idempotent (FR-4) and Syncthing runs detached in its own session, so
//! a run-once-at-login job models "the agent is up" without the init system
//! needing to track the Syncthing pid. The trade-off — a crashed Syncthing is
//! not auto-restarted — is acceptable for the MVP; `byteferret doctor` surfaces
//! a dead instance and `byteferret start` revives it.

/// Render the unit file for a byteferret binary living at `exe`.
///
/// `exe` should be an absolute path so the unit keeps working regardless of the
/// caller's `PATH` (systemd runs with a minimal environment).
#[cfg_attr(target_os = "macos", allow(dead_code))] // used on Linux; kept testable everywhere
pub fn unit_contents(exe: &str) -> String {
    format!(
        "[Unit]\n\
         Description=ByteFerret P2P document vault agent\n\
         Documentation=https://github.com/gevious/byteferret\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart={exe} start\n\
         ExecStop={exe} stop\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Bare unit name used with `systemctl --user`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const UNIT_NAME: &str = "byteferret.service";

/// launchd job label — also the plist's file stem under ~/Library/LaunchAgents.
pub const LAUNCHD_LABEL: &str = "com.byteferret.agent";

/// Minimal XML text escaping for values embedded in the plist (the install
/// prefix is user-controlled, so `&` or `<` in a path must not break the XML).
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Render the launchd user-agent plist for a byteferret binary living at `exe`.
///
/// `RunAtLoad` makes launchd run `byteferret start` when the agent is loaded
/// (login, or `launchctl bootstrap` at install time). `KeepAlive=false` because
/// `start` is a oneshot that detaches Syncthing and exits.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // used on macOS; kept testable everywhere
pub fn plist_contents(exe: &str) -> String {
    let exe = xml_escape(exe);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_wires_start_and_stop_to_the_given_binary() {
        let u = unit_contents("/home/alice/.local/bin/byteferret");
        assert!(u.contains("ExecStart=/home/alice/.local/bin/byteferret start"));
        assert!(u.contains("ExecStop=/home/alice/.local/bin/byteferret stop"));
        // oneshot + RemainAfterExit is what keeps the service "active" after
        // ExecStart returns (Syncthing having been detached).
        assert!(u.contains("Type=oneshot"));
        assert!(u.contains("RemainAfterExit=yes"));
        assert!(u.contains("WantedBy=default.target"));
    }

    #[test]
    fn plist_wires_start_and_runs_at_load() {
        let p = plist_contents("/Users/alice/.local/bin/byteferret");
        assert!(p.contains("<string>/Users/alice/.local/bin/byteferret</string>"));
        assert!(p.contains("<string>start</string>"));
        assert!(p.contains("<string>com.byteferret.agent</string>"));
        // RunAtLoad + no KeepAlive: run the oneshot `start` at login, don't respawn.
        assert!(p.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(p.contains("<key>KeepAlive</key>\n    <false/>"));
    }

    #[test]
    fn plist_escapes_xml_specials_in_the_binary_path() {
        let p = plist_contents("/Users/a&b/<odd>/byteferret");
        assert!(p.contains("<string>/Users/a&amp;b/&lt;odd&gt;/byteferret</string>"));
    }
}
