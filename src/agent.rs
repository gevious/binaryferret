//! Orchestration facade the CLI commands build on. Owns the wiring between the
//! agent's persisted state (config + secrets), the managed Syncthing process,
//! and the REST client — so commands express intent, not plumbing.

use anyhow::Result;
use serde_json::{json, Value};

use crate::config::{Config, Secrets};
use crate::output::say;
use crate::paths::Paths;
use crate::syncthing::process;
use crate::syncthing::rest::Client;

pub struct Context {
    pub paths: Paths,
    pub config: Config,
    pub api_key: String,
    pub client: Client,
}

/// Ensures Syncthing is up and returns a ready Context. Persists the generated
/// API key (secrets, 0600) and effective settings (config) so later commands
/// and restarts converge to the same instance (FR-4 idempotency).
pub fn ensure_started() -> Result<Context> {
    let paths = Paths::resolve();
    let config = Config::load(&paths)?;
    let secrets = Secrets::load(&paths)?;

    let res = process::start(&paths, &config.gui_address, secrets.syncthing_api_key.clone())?;

    if secrets.syncthing_api_key.as_deref() != Some(res.api_key.as_str()) {
        Secrets { syncthing_api_key: Some(res.api_key.clone()) }.save(&paths)?;
    }
    config.save(&paths)?;

    let client = Client::new(&res.gui_address, &res.api_key);
    if !res.already_running {
        apply_runtime_toggles(&client)?;
    }
    Ok(Context { paths, config, api_key: res.api_key, client })
}

/// Load a Context without (re)starting — for read-only commands.
pub fn load_context() -> Result<Context> {
    let paths = Paths::resolve();
    let config = Config::load(&paths)?;
    let secrets = Secrets::load(&paths)?;
    let api_key = secrets.syncthing_api_key.unwrap_or_default();
    let client = Client::new(&config.gui_address, &api_key);
    Ok(Context { paths, config, api_key, client })
}

/// Applies privacy / connectivity toggles that map to documented options:
///  - BYTEFERRET_SYNC_ADDRESS       → pin Syncthing's sync listen address (also
///    lets two isolated agents coexist on one host for testing).
///  - BYTEFERRET_DISCOVERY_PUBLIC=false → the getting-started `discovery.public`
///    off switch: no global/local announce, no relays (LAN/VPN only).
///
/// Both are no-ops when unset, so real desktops keep Syncthing's defaults.
fn apply_runtime_toggles(client: &Client) -> Result<()> {
    let sync_addr = std::env::var("BYTEFERRET_SYNC_ADDRESS").ok().filter(|s| !s.is_empty());
    let public = std::env::var("BYTEFERRET_DISCOVERY_PUBLIC").ok();
    if sync_addr.is_none() && public.is_none() {
        return Ok(());
    }
    let mut opts = client.get_options()?;
    if let Some(addr) = sync_addr {
        opts["listenAddresses"] = json!([addr]);
    }
    if public.as_deref() == Some("false") {
        opts["globalAnnounceEnabled"] = json!(false);
        opts["localAnnounceEnabled"] = json!(false);
        opts["relaysEnabled"] = json!(false);
    }
    client.put_options(&opts)?;
    Ok(())
}

/// Adds a peer device and shares the vault folder with it — the shared core of
/// `pair --with` and `pair --accept`. Idempotent: re-adding an existing peer or
/// re-sharing an existing folder membership makes no change.
pub fn add_peer(
    client: &Client,
    folder_id: &str,
    device_id: &str,
    name: Option<&str>,
    address: Option<&str>,
) -> Result<()> {
    let existing = client
        .get_devices()?
        .into_iter()
        .find(|d| d.get("deviceID").and_then(Value::as_str) == Some(device_id));

    let name = name
        .map(str::to_string)
        .or_else(|| existing.as_ref().and_then(|d| d.get("name")).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| device_id.chars().take(7).collect());

    let addresses: Vec<String> = match address {
        Some(a) => vec![a.to_string()],
        None => existing
            .as_ref()
            .and_then(|d| d.get("addresses"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| vec!["dynamic".to_string()]),
    };

    // FR-7: never auto-accept unknown folders; never act as introducer.
    let device = json!({
        "deviceID": device_id,
        "name": name,
        "addresses": addresses,
        "autoAcceptFolders": false,
        "introducer": false,
    });
    client.put_device(&device)?;

    match client.get_folder(folder_id)? {
        None => say(&format!("(no vault folder '{folder_id}' yet — run `byteferret init` first; peer added regardless)")),
        Some(mut folder) => {
            let already = folder
                .get("devices")
                .and_then(Value::as_array)
                .map(|a| a.iter().any(|d| d.get("deviceID").and_then(Value::as_str) == Some(device_id)))
                .unwrap_or(false);
            if !already {
                if let Some(arr) = folder.get_mut("devices").and_then(Value::as_array_mut) {
                    arr.push(json!({ "deviceID": device_id }));
                    client.put_folder(&folder)?;
                }
            }
        }
    }
    Ok(())
}

/// Standard folder settings for the vault (near-real-time sync, FR-6/FR-9/FR-17).
pub fn vault_folder_config(id: &str, label: &str, path: &str, peers: &[String]) -> Value {
    let devices: Vec<Value> = peers.iter().map(|p| json!({ "deviceID": p })).collect();
    json!({
        "id": id,
        "label": label,
        "path": path,
        "type": "sendreceive",
        "fsWatcherEnabled": true,
        "fsWatcherDelayS": 5,
        "rescanIntervalS": 60,
        "devices": devices,
        "versioning": { "type": "" }, // v1: minimal on desktop; the hub is canonical history
    })
}
