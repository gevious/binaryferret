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

/// Reduce a peer-supplied folder label or id to a single safe directory name.
///
/// When we accept a folder a peer offered, its label and id are strings that
/// *the peer chose*, and we are about to turn one into a directory we create.
/// Anything that could steer that outside the intended parent — `..`, a path
/// separator, an absolute path, a leading `~` — has to be gone, not escaped, so
/// the result is always exactly one ordinary component. Returns `None` when
/// nothing usable survives, in which case the caller should ask for an explicit
/// `--path` rather than invent one.
pub fn safe_dir_name(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c == ' ' || c == '.' {
            // Keep words separated without ever emitting a `.` (so `..` and
            // hidden `.names` are unrepresentable).
            out.push('-');
        }
        // Everything else — separators, control characters, non-ASCII — is dropped.
    }
    let out = out.trim_matches('-').to_string();
    let out: String = out.chars().take(64).collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
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

#[cfg(test)]
mod tests {
    use super::safe_dir_name;

    #[test]
    fn ordinary_names_survive() {
        assert_eq!(safe_dir_name("Work Notes").as_deref(), Some("Work-Notes"));
        assert_eq!(safe_dir_name("byteferret-vault").as_deref(), Some("byteferret-vault"));
    }

    #[test]
    fn traversal_and_absolute_paths_cannot_survive() {
        // A hostile label must never yield anything with a separator or `..`.
        for hostile in [
            "../../.ssh",
            "/etc/cron.d",
            "..",
            ".",
            "~/.bashrc",
            "a/../../b",
            ".hidden",
        ] {
            let got = safe_dir_name(hostile);
            if let Some(g) = &got {
                assert!(!g.contains('/'), "{hostile} → {g}");
                assert!(!g.contains('.'), "{hostile} → {g}");
                assert!(!g.starts_with('-'), "{hostile} → {g}");
                assert_ne!(g, "..");
            }
        }
        assert_eq!(safe_dir_name(".."), None);
        assert_eq!(safe_dir_name("/"), None);
    }

    #[test]
    fn unusable_labels_yield_none() {
        assert_eq!(safe_dir_name(""), None);
        assert_eq!(safe_dir_name("///"), None);
        assert_eq!(safe_dir_name("\u{1b}\u{7f}"), None);
    }

    #[test]
    fn control_characters_are_stripped_from_the_name() {
        // Whatever survives must be an ordinary component — here the escape
        // sequence's punctuation is dropped and only plain characters remain.
        assert_eq!(safe_dir_name("\u{1b}[31mred").as_deref(), Some("31mred"));
    }

    #[test]
    fn names_are_length_capped() {
        assert!(safe_dir_name(&"a".repeat(500)).unwrap().len() <= 64);
    }
}
