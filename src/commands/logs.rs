//! `byteferret logs` — show the managed Syncthing's log (FR-26). The agent's
//! only log today is `~/.local/share/byteferret/syncthing.log` (stdout+stderr of
//! the detached Syncthing process); this command tails it without the user
//! needing to know that path.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::json;

use crate::output::{emit, is_json_mode, say};
use crate::paths::Paths;

/// Return the last `n` lines of `text` (all of it if it has fewer).
fn tail_lines(text: &str, n: usize) -> Vec<String> {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(n);
    all[start..].iter().map(|s| s.to_string()).collect()
}

pub fn logs(lines: usize, follow: bool, path_only: bool) -> Result<()> {
    let paths = Paths::resolve();
    let log = &paths.log_file;

    if path_only {
        say(&log.display().to_string());
        emit(&json!({ "ok": true, "path": log.to_string_lossy() }));
        return Ok(());
    }

    if follow && is_json_mode() {
        bail!("--follow streams and cannot be combined with --json");
    }

    if !log.exists() {
        say(&format!(
            "no log yet at {} — start the agent with `byteferret start`",
            log.display()
        ));
        emit(&json!({ "ok": true, "path": log.to_string_lossy(), "exists": false, "lines": [] }));
        return Ok(());
    }

    let text = std::fs::read_to_string(log)?;
    let tail = tail_lines(&text, lines);

    if is_json_mode() {
        emit(&json!({ "ok": true, "path": log.to_string_lossy(), "exists": true, "lines": tail }));
        return Ok(());
    }
    for line in &tail {
        println!("{line}");
    }

    if follow {
        follow_from(log, text.len() as u64)?;
    }
    Ok(())
}

/// Poll the log for appended bytes and echo them, starting at byte `offset`.
/// Simple by design (no inotify) so the static musl binary stays dependency-free.
fn follow_from(log: &std::path::Path, mut offset: u64) -> Result<()> {
    let mut f = File::open(log)?;
    let stdout = std::io::stdout();
    loop {
        let len = f.metadata()?.len();
        if len < offset {
            // File was truncated/rotated — restart from the top.
            offset = 0;
        }
        if len > offset {
            f.seek(SeekFrom::Start(offset))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            offset += buf.len() as u64;
            let mut lock = stdout.lock();
            lock.write_all(&buf)?;
            lock.flush()?;
        }
        sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::tail_lines;

    #[test]
    fn tail_returns_last_n_lines() {
        let text = "a\nb\nc\nd\ne\n";
        assert_eq!(tail_lines(text, 2), vec!["d", "e"]);
    }

    #[test]
    fn tail_returns_everything_when_fewer_than_n() {
        let text = "one\ntwo\n";
        assert_eq!(tail_lines(text, 10), vec!["one", "two"]);
    }

    #[test]
    fn tail_of_empty_is_empty() {
        assert!(tail_lines("", 5).is_empty());
    }
}
