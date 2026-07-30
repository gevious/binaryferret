//! The agent's own state, split into two files:
//!  - config.toml  — shareable, human-editable (vault path, folder id, gui bind)
//!  - secrets      — 0600, never logged (Syncthing REST API key)

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::{gui_address_override, Paths, DEFAULT_FOLDER_ID, DEFAULT_GUI_ADDRESS};

/// On-disk shape of config.toml (all optional; defaults applied on load).
#[derive(Default, Serialize, Deserialize)]
struct RawConfig {
    gui_address: Option<String>,
    vault_path: Option<String>,
    folder_id: Option<String>,
}

/// Resolved agent configuration.
#[derive(Clone)]
pub struct Config {
    pub gui_address: String,
    pub vault_path: Option<String>,
    pub folder_id: String,
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
            vault_path: raw.vault_path,
            folder_id: raw.folder_id.unwrap_or_else(|| DEFAULT_FOLDER_ID.to_string()),
        })
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        ensure_dir(&paths.config_dir)?;
        let raw = RawConfig {
            gui_address: Some(self.gui_address.clone()),
            vault_path: self.vault_path.clone(),
            folder_id: Some(self.folder_id.clone()),
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
        fs::write(&paths.secrets_file, body)?;
        fs::set_permissions(&paths.secrets_file, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))
}
