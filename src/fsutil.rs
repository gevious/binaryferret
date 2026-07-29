//! Small filesystem helpers.

use std::path::{Path, PathBuf};

/// Expand a leading `~` and make the path absolute.
pub fn expand_path(p: &str) -> PathBuf {
    let expanded = if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(expanded)
    }
}

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// Recursively find files whose name contains `marker`, skipping Syncthing/agent
/// internal dot-dirs. Bounded by `limit`.
pub fn find_files(root: &Path, marker: &str, limit: usize) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    walk(root, marker, limit, &mut hits);
    hits
}

fn walk(dir: &Path, marker: &str, limit: usize, hits: &mut Vec<PathBuf>) {
    if hits.len() >= limit {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if hits.len() >= limit {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if path.is_dir() {
            if name.starts_with(".st") || name == ".git" {
                continue;
            }
            walk(&path, marker, limit, hits);
        } else if name.contains(marker) {
            hits.push(path);
        }
    }
}
