//! The agent's own state, split into two files:
//!  - config.toml  — shareable, human-editable (gui bind, device aliases)
//!  - secrets      — 0600, never logged (Syncthing REST API key)

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::{gui_address_override, Paths, DEFAULT_GUI_ADDRESS};

/// On-disk shape of config.toml (all optional; defaults applied on load).
///
/// Older versions also stored `vault_path`/`folder_id` for a privileged "vault"
/// folder; there is no longer a special folder, so those keys are simply ignored
/// if present (serde drops unknown fields), and never written back.
#[derive(Default, Serialize, Deserialize)]
struct RawConfig {
    gui_address: Option<String>,
    /// Local, user-chosen labels for device ids (device id → alias).
    aliases: Option<BTreeMap<String, String>>,
}

/// Resolved agent configuration.
#[derive(Clone)]
pub struct Config {
    pub gui_address: String,
    /// User-set aliases, keyed by device id as typed. Unlike a peer's Syncthing
    /// `name` (which the remote machine chooses), an alias is chosen locally, so
    /// it is trusted enough to resolve a pairing target by. Compared with dashes
    /// and case ignored, so the stored key format doesn't matter.
    pub aliases: BTreeMap<String, String>,
}

/// Device ids are uppercase base32 whose dashes are only visual grouping, so
/// neither case nor dashes are significant when comparing them.
fn norm(s: &str) -> String {
    s.chars().filter(|c| *c != '-').flat_map(char::to_uppercase).collect()
}

impl Config {
    pub fn load(paths: &Paths) -> Result<Config> {
        let raw: RawConfig = if paths.config_file.exists() {
            let text = fs::read_to_string(&paths.config_file)
                .with_context(|| format!("reading {}", paths.config_file.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", paths.config_file.display()))?
        } else {
            RawConfig::default()
        };
        Ok(Config {
            // env override always wins so an isolated test instance can pick its port
            gui_address: gui_address_override()
                .or(raw.gui_address)
                .unwrap_or_else(|| DEFAULT_GUI_ADDRESS.to_string()),
            aliases: raw.aliases.unwrap_or_default(),
        })
    }

    /// The alias set for `device_id`, if any (dashes/case ignored).
    pub fn alias_for(&self, device_id: &str) -> Option<&str> {
        let n = norm(device_id);
        self.aliases.iter().find(|(k, _)| norm(k) == n).map(|(_, v)| v.as_str())
    }

    /// The device id an alias points to (alias compared case-insensitively).
    pub fn device_for_alias(&self, alias: &str) -> Option<&str> {
        self.aliases.iter().find(|(_, v)| v.eq_ignore_ascii_case(alias)).map(|(k, _)| k.as_str())
    }

    /// Set (or replace) the alias for a device id. Any prior alias for the same
    /// device — regardless of the exact key format it was stored under — is
    /// dropped first so a device never carries two aliases.
    pub fn set_alias(&mut self, device_id: String, alias: String) {
        let n = norm(&device_id);
        self.aliases.retain(|k, _| norm(k) != n);
        self.aliases.insert(device_id, alias);
    }

    /// Remove any alias for `device_id`. Returns whether one was removed.
    pub fn remove_alias(&mut self, device_id: &str) -> bool {
        let n = norm(device_id);
        let before = self.aliases.len();
        self.aliases.retain(|k, _| norm(k) != n);
        self.aliases.len() != before
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        ensure_dir(&paths.config_dir)?;
        let raw = RawConfig {
            gui_address: Some(self.gui_address.clone()),
            aliases: if self.aliases.is_empty() { None } else { Some(self.aliases.clone()) },
        };
        let body = format!(
            "# ByteFerret agent configuration — safe to edit and share (no secrets here).\n\n{}",
            toml::to_string(&raw)?
        );
        fs::write(&paths.config_file, body)
            .with_context(|| format!("writing {}", paths.config_file.display()))?;
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct RawSecrets {
    syncthing_api_key: Option<String>,
}

#[derive(Default)]
pub struct Secrets {
    pub syncthing_api_key: Option<String>,
}

impl Secrets {
    pub fn load(paths: &Paths) -> Result<Secrets> {
        if !paths.secrets_file.exists() {
            return Ok(Secrets::default());
        }
        let text = fs::read_to_string(&paths.secrets_file)?;
        let raw: RawSecrets = toml::from_str(&text)?;
        Ok(Secrets { syncthing_api_key: raw.syncthing_api_key })
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        ensure_dir(&paths.config_dir)?;
        let raw = RawSecrets { syncthing_api_key: self.syncthing_api_key.clone() };
        let body = format!(
            "# ByteFerret secrets — DO NOT SHARE. Permissions are enforced to 0600.\n\n{}",
            toml::to_string(&raw)?
        );
        // Create the file already at 0600 rather than writing it and tightening
        // afterwards: the write-then-chmod order leaves the API key readable by
        // any local user for the moment in between, which is long enough for a
        // watcher on the config dir to win the race.
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&paths.secrets_file)
            .with_context(|| format!("writing {}", paths.secrets_file.display()))?;
        // `mode()` only applies when the file is created, so an existing file
        // keeps whatever (possibly loose) mode it had — tighten it before the
        // new key lands in it.
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
        f.write_all(body.as_bytes())?;
        Ok(())
    }
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))
}

#[cfg(test)]
mod alias_tests {
    use super::*;

    fn cfg() -> Config {
        Config { gui_address: String::new(), aliases: BTreeMap::new() }
    }

    #[test]
    fn set_then_look_up_both_ways() {
        let mut c = cfg();
        c.set_alias("AAAAAAA-BBBBBBB".into(), "laptop".into());
        assert_eq!(c.alias_for("AAAAAAA-BBBBBBB"), Some("laptop"));
        assert_eq!(c.device_for_alias("laptop"), Some("AAAAAAA-BBBBBBB"));
    }

    #[test]
    fn lookups_ignore_dashes_and_case() {
        let mut c = cfg();
        c.set_alias("AAAAAAA-BBBBBBB".into(), "laptop".into());
        assert_eq!(c.alias_for("aaaaaaabbbbbbb"), Some("laptop"));
        assert_eq!(c.device_for_alias("LAPTOP"), Some("AAAAAAA-BBBBBBB"));
    }

    #[test]
    fn setting_again_replaces_rather_than_duplicates() {
        let mut c = cfg();
        c.set_alias("AAAAAAA-BBBBBBB".into(), "laptop".into());
        // Same device, different key format — must not leave two entries behind.
        c.set_alias("aaaaaaa-bbbbbbb".into(), "work".into());
        assert_eq!(c.aliases.len(), 1);
        assert_eq!(c.alias_for("AAAAAAA-BBBBBBB"), Some("work"));
    }

    #[test]
    fn remove_reports_whether_anything_went() {
        let mut c = cfg();
        c.set_alias("AAAAAAA-BBBBBBB".into(), "laptop".into());
        assert!(c.remove_alias("aaaaaaabbbbbbb"));
        assert!(!c.remove_alias("AAAAAAA-BBBBBBB"));
        assert_eq!(c.alias_for("AAAAAAA-BBBBBBB"), None);
    }
}
