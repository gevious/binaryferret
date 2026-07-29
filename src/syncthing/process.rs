//! Manages the lifecycle of the agent's private Syncthing instance: one-time
//! key/config generation, starting it as a detached background process bound to
//! localhost with our own API key, health polling, and shutdown.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::binary::ensure_binary;
use super::rest::Client;
use crate::paths::Paths;

fn random_api_key() -> Result<String> {
    let mut buf = [0u8; 24];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

/// True if a process with this pid is alive (kill(pid, 0)).
fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

fn read_pid(pid_file: &Path) -> Option<i32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

/// One-time: generate Syncthing's device keys + base config if absent.
fn ensure_generated(bin: &Path, home: &Path) -> Result<()> {
    if home.join("config.xml").exists() {
        return Ok(());
    }
    fs::create_dir_all(home)?;
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))?;
    let out = Command::new(bin)
        .args(["generate", "--home"])
        .arg(home)
        .args(["--no-default-folder", "--skip-port-probing"])
        .output()
        .context("running `syncthing generate`")?;
    if !out.status.success() {
        bail!("syncthing generate failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub struct StartResult {
    pub api_key: String,
    pub gui_address: String,
    pub already_running: bool,
}

/// Ensures Syncthing is running. Idempotent: if a healthy instance is already
/// up, returns it. Otherwise (re)starts it bound to `gui_address` (localhost)
/// with `api_key`, writing a pidfile.
pub fn start(paths: &Paths, gui_address: &str, existing_api_key: Option<String>) -> Result<StartResult> {
    let api_key = match existing_api_key {
        Some(k) => k,
        None => random_api_key()?,
    };
    let client = Client::new(gui_address, &api_key);

    if let Some(pid) = read_pid(&paths.pid_file) {
        if pid_alive(pid) && client.ping() {
            return Ok(StartResult { api_key, gui_address: gui_address.to_string(), already_running: true });
        }
    }

    let bin = ensure_binary(&paths.syncthing_bin)?;
    ensure_generated(&bin, &paths.syncthing_home)?;
    fs::create_dir_all(&paths.data_dir)?;

    let log = OpenOptions::new().create(true).append(true).open(&paths.log_file)?;
    let log_err = log.try_clone()?;

    let mut cmd = Command::new(&bin);
    cmd.args(["serve", "--home"])
        .arg(&paths.syncthing_home)
        .args(["--no-browser", "--no-restart"])
        .env("STGUIAPIKEY", &api_key)
        .env("STGUIADDRESS", gui_address)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // Detach into its own session so it survives the CLI process and terminal.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("spawning syncthing")?;
    fs::write(&paths.pid_file, format!("{}\n", child.id()))?;

    wait_until_ready(&client, &paths.log_file, Duration::from_secs(30))?;
    Ok(StartResult { api_key, gui_address: gui_address.to_string(), already_running: false })
}

fn wait_until_ready(client: &Client, log_file: &Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if client.ping() {
            return Ok(());
        }
        sleep(Duration::from_millis(500));
    }
    let tail = fs::read_to_string(log_file)
        .map(|s| s.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    bail!("Syncthing did not become ready within {}s. Recent log:\n{tail}", timeout.as_secs())
}

pub struct StopResult {
    pub was_running: bool,
}

/// Stops the managed Syncthing if running. Idempotent.
pub fn stop(paths: &Paths) -> Result<StopResult> {
    let pid = match read_pid(&paths.pid_file) {
        Some(p) if pid_alive(p) => p,
        _ => {
            let _ = fs::remove_file(&paths.pid_file);
            return Ok(StopResult { was_running: false });
        }
    };
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for _ in 0..20 {
        if !pid_alive(pid) {
            break;
        }
        sleep(Duration::from_millis(250));
    }
    if pid_alive(pid) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    let _ = fs::remove_file(&paths.pid_file);
    Ok(StopResult { was_running: true })
}

/// Whether the managed Syncthing appears to be running (pid + REST ping).
pub fn is_running(paths: &Paths, gui_address: &str, api_key: Option<&str>) -> bool {
    let Some(pid) = read_pid(&paths.pid_file) else { return false };
    let Some(key) = api_key else { return false };
    pid_alive(pid) && Client::new(gui_address, key).ping()
}
