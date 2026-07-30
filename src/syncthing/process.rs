//! Manages the lifecycle of the agent's private Syncthing instance: one-time
//! key/config generation, starting it as a detached background process bound to
//! localhost with our own API key, health polling, and shutdown.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
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

/// Parent pid of `pid`, read from `/proc/<pid>/stat`. The `comm` field can hold
/// spaces and parens, so we split after the last ')' before reading the numeric
/// fields (`state ppid …`).
#[cfg(target_os = "linux")]
fn ppid(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    let mut fields = after.split_whitespace();
    fields.next()?; // state
    fields.next()?.parse().ok() // ppid
}

/// Parent pid of `pid` via `ps` — the portable fallback where there is no
/// `/proc` (macOS). `ps -o ppid= -p <pid>` prints just the ppid (or nothing if
/// the process is gone).
#[cfg(not(target_os = "linux"))]
fn ppid(pid: i32) -> Option<i32> {
    let out = Command::new("ps")
        .args(["-o", "ppid=", "-p"])
        .arg(pid.to_string())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Whether a process's command line is one of *our* Syncthings: a `syncthing`
/// binary running the `serve` verb against our exact `--home`.
///
/// This is the one definition of "ours", shared by the `/proc` scan and by the
/// pidfile check, so a pid can never be acted on through one path that the other
/// would reject. It matters because the alternative — trusting any live pid the
/// pidfile names — will eventually point at an unrelated process: pids are
/// recycled, and a stale pidfile that survives a reboot can name anything.
#[cfg(target_os = "linux")]
fn is_managed(pid: i32, home: &Path) -> bool {
    let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else { return false };
    // cmdline is NUL-separated argv, so args with spaces compare exactly.
    let args: Vec<&[u8]> = raw.split(|b| *b == 0).filter(|a| !a.is_empty()).collect();
    let Some(argv0) = args.first() else { return false };
    argv0.ends_with(b"/syncthing")
        && args.iter().any(|a| *a == b"serve")
        && args.iter().any(|a| *a == home.as_os_str().as_bytes())
}

/// macOS has no `/proc`, so the same question is answered from `ps`. The command
/// line arrives as one flattened string, so we match the composed `--home <path>`
/// rather than a bare path substring — otherwise any process merely *mentioning*
/// our home directory would look like ours.
#[cfg(not(target_os = "linux"))]
fn cmd_is_managed(cmd: &str, home: &Path) -> bool {
    cmd.contains("/syncthing ")
        && cmd.contains(" serve")
        && cmd.contains(&format!("--home {}", home.display()))
}

#[cfg(not(target_os = "linux"))]
fn is_managed(pid: i32, home: &Path) -> bool {
    let Ok(out) = Command::new("ps").args(["-ww", "-o", "command=", "-p"]).arg(pid.to_string()).output() else {
        return false;
    };
    cmd_is_managed(String::from_utf8_lossy(&out.stdout).trim(), home)
}

/// Every running Syncthing that is *ours*, found by scanning `/proc` for the
/// unique `serve --home <our home>` command line. This is the fallback the
/// pidfile can't provide: after a crash, a reinstall, or a lost pidfile, the
/// pidfile may name a dead or wrong process while the real Syncthing keeps
/// running and holding ports 8384/22000. Matching on the home path (stable
/// across restarts and independent of the API key) finds it regardless of what
/// the pidfile says. Returns both the monitor and worker process when present.
#[cfg(target_os = "linux")]
fn managed_pids(home: &Path) -> Vec<i32> {
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else { return pids };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        if is_managed(pid, home) {
            pids.push(pid);
        }
    }
    pids
}

/// macOS has no `/proc`, so we enumerate processes with `ps` and match on the
/// command line instead — the same signal (the `serve` verb plus our unique
/// `--home <path>`), just sourced differently. `-ww` disables ps's column
/// truncation so long home paths aren't clipped away from the match.
#[cfg(not(target_os = "linux"))]
fn managed_pids(home: &Path) -> Vec<i32> {
    let mut pids = Vec::new();
    let Ok(out) = Command::new("ps").args(["-axww", "-o", "pid=,command="]).output() else {
        return pids;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let line = line.trim_start();
        let Some((pid_str, cmd)) = line.split_once(char::is_whitespace) else { continue };
        let Ok(pid) = pid_str.parse::<i32>() else { continue };
        if cmd_is_managed(cmd, home) {
            pids.push(pid);
        }
    }
    pids
}

/// The top-most (monitor) pid in a managed set — the one whose parent isn't also
/// in the set — so the pidfile keeps naming the supervisor, as a fresh spawn
/// does. Falls back to the lowest pid if the parentage can't be read.
fn supervisor_pid(pids: &[i32]) -> Option<i32> {
    pids.iter()
        .copied()
        .find(|&p| ppid(p).map(|pp| !pids.contains(&pp)).unwrap_or(true))
        .or_else(|| pids.iter().copied().min())
}

/// Terminate a set of pids: SIGTERM, wait up to ~5s for a clean exit, then
/// SIGKILL any survivor. Safe on an empty set and on pids that are already gone.
fn terminate(pids: &[i32]) {
    for &pid in pids {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    for _ in 0..20 {
        if pids.iter().all(|&p| !pid_alive(p)) {
            return;
        }
        sleep(Duration::from_millis(250));
    }
    for &pid in pids {
        if pid_alive(pid) {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
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

    // Fast path: our pidfile names one of our Syncthings and it answers the REST API.
    if let Some(pid) = read_pid(&paths.pid_file) {
        if is_managed(pid, &paths.syncthing_home) && client.ping() {
            return Ok(StartResult { api_key, gui_address: gui_address.to_string(), already_running: true });
        }
    }

    // Pidfile fallback: the pidfile is stale (crash, reinstall, or it was
    // deleted) but a Syncthing of ours may still be running and holding the
    // ports — which is exactly what makes a naive spawn fail with "address
    // already in use". Find it by home path instead of trusting the pidfile.
    let running = managed_pids(&paths.syncthing_home);
    if !running.is_empty() {
        if client.ping() {
            // Healthy and reachable: adopt it and repair the pidfile so later
            // `stop`/`status` track the right process.
            if let Some(sup) = supervisor_pid(&running) {
                let _ = fs::write(&paths.pid_file, format!("{sup}\n"));
            }
            return Ok(StartResult { api_key, gui_address: gui_address.to_string(), already_running: true });
        }
        // Ours, but unreachable — typically its API key no longer matches (a
        // reinstall regenerated our secrets). It can only fight us for the
        // ports, so retire it before starting fresh.
        terminate(&running);
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

/// Stops the managed Syncthing if running. Idempotent. Kills every managed
/// process found by scanning — not just the pidfile's — so a stale or missing
/// pidfile can't strand an orphan that keeps holding the ports (the failure
/// where `stop` reports "not running" yet the next `start` can't bind).
pub fn stop(paths: &Paths) -> Result<StopResult> {
    let mut pids = managed_pids(&paths.syncthing_home);
    // Include the pidfile's process too, in case it somehow isn't matched by the
    // scan (e.g. an unreadable /proc entry) — but only after confirming it really
    // is our Syncthing. Liveness alone is not enough: pids get recycled, so a
    // pidfile left behind by a crash or a reboot can name an unrelated process of
    // ours, and killing it would be indistinguishable from `stop` working.
    if let Some(p) = read_pid(&paths.pid_file) {
        if is_managed(p, &paths.syncthing_home) && !pids.contains(&p) {
            pids.push(p);
        }
    }
    let was_running = !pids.is_empty();
    terminate(&pids);
    let _ = fs::remove_file(&paths.pid_file);
    Ok(StopResult { was_running })
}

/// Whether the managed Syncthing appears to be running: a process of ours is
/// alive (per pidfile or scan) and answers the REST API. The scan makes this
/// robust to a stale/missing pidfile.
pub fn is_running(paths: &Paths, gui_address: &str, api_key: Option<&str>) -> bool {
    let Some(key) = api_key else { return false };
    let alive = read_pid(&paths.pid_file).map(|p| is_managed(p, &paths.syncthing_home)).unwrap_or(false)
        || !managed_pids(&paths.syncthing_home).is_empty();
    alive && Client::new(gui_address, key).ping()
}
