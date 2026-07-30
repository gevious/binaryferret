//! Resolves every on-disk location the agent uses, honoring XDG and a set of
//! BYTEFERRET_* overrides. The overrides let a second, fully isolated agent run
//! on the same machine (for local testing) without touching the first's config,
//! Syncthing home, or REST port.

use std::env;
use std::path::PathBuf;

/// Pinned Syncthing release the agent downloads and manages.
pub const SYNCTHING_VERSION: &str = "1.30.0";

/// Pinned Typst release used for local `publish --pdf` rendering (FR-19).
pub const TYPST_VERSION: &str = "0.12.0";

/// Folder ID shared by every machine's default vault so `pair` links them.
pub const DEFAULT_FOLDER_ID: &str = "byteferret-vault";

/// Default localhost bind for Syncthing's REST/GUI API.
pub const DEFAULT_GUI_ADDRESS: &str = "127.0.0.1:8384";

fn env_nonempty(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

fn home() -> PathBuf {
    env_nonempty("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(name: &str, fallback: &str) -> PathBuf {
    env_nonempty(name).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}

#[derive(Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub secrets_file: PathBuf,
    pub syncthing_home: PathBuf,
    pub syncthing_bin: PathBuf,
    pub typst_bin: PathBuf,
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
}

impl Paths {
    pub fn resolve() -> Paths {
        let config_dir = env_nonempty("BYTEFERRET_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| xdg("XDG_CONFIG_HOME", ".config").join("byteferret"));
        let data_dir = env_nonempty("BYTEFERRET_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share").join("byteferret"));
        Paths {
            config_file: config_dir.join("config.toml"),
            secrets_file: config_dir.join("secrets"),
            syncthing_home: data_dir.join("syncthing"),
            syncthing_bin: data_dir.join("bin").join("syncthing"),
            typst_bin: data_dir.join("bin").join("typst"),
            pid_file: data_dir.join("syncthing.pid"),
            log_file: data_dir.join("syncthing.log"),
            config_dir,
            data_dir,
        }
    }
}

/// Explicit GUI-address override (config value wins if this is unset).
pub fn gui_address_override() -> Option<String> {
    env_nonempty("BYTEFERRET_GUI_ADDRESS")
}

/// Path of the systemd *user* unit the agent installs (FR-1). Follows
/// XDG_CONFIG_HOME and is intentionally independent of BYTEFERRET_CONFIG_DIR —
/// systemd only reads units from its standard `systemd/user/` location.
pub fn systemd_user_unit() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("systemd/user/byteferret.service")
}
