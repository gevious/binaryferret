//! systemd user-service integration (FR-1). BinaryFerret installs itself as a
//! `Type=oneshot` **user** service with `RemainAfterExit=yes`:
//!   ExecStart = `binaryferret start`  (brings the managed Syncthing up, detached)
//!   ExecStop  = `binaryferret stop`   (takes it back down)
//!
//! We deliberately avoid a long-lived foreground supervisor: `start`/`stop` are
//! already idempotent (FR-4) and Syncthing runs detached in its own session, so
//! oneshot+RemainAfterExit models "the agent is up" without systemd needing to
//! track the Syncthing pid. The trade-off — systemd won't auto-restart a crashed
//! Syncthing — is acceptable for the MVP; `binaryferret doctor` surfaces a dead
//! instance and `binaryferret start` revives it.

/// Render the unit file for a binaryferret binary living at `exe`.
///
/// `exe` should be an absolute path so the unit keeps working regardless of the
/// caller's `PATH` (systemd runs with a minimal environment).
pub fn unit_contents(exe: &str) -> String {
    format!(
        "[Unit]\n\
         Description=BinaryFerret P2P document vault agent\n\
         Documentation=https://github.com/binaryferret/binaryferret\n\
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
pub const UNIT_NAME: &str = "binaryferret.service";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_wires_start_and_stop_to_the_given_binary() {
        let u = unit_contents("/home/alice/.local/bin/binaryferret");
        assert!(u.contains("ExecStart=/home/alice/.local/bin/binaryferret start"));
        assert!(u.contains("ExecStop=/home/alice/.local/bin/binaryferret stop"));
        // oneshot + RemainAfterExit is what keeps the service "active" after
        // ExecStart returns (Syncthing having been detached).
        assert!(u.contains("Type=oneshot"));
        assert!(u.contains("RemainAfterExit=yes"));
        assert!(u.contains("WantedBy=default.target"));
    }
}
