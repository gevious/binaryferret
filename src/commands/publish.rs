//! `byteferret publish <file> [--pdf] [--out <path>] [--email]` — render a vault
//! Markdown document to a PDF locally (FR-19) using the bundled Typst, and
//! optionally open a mail draft with it attached via `xdg-email` (FR-20).
//!
//! Fully offline and hub-less: the only network use is the one-time Typst
//! download on first run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::fsutil::expand_path;
use crate::output::{emit, say};
use crate::paths::Paths;
use crate::publish::{markdown, typst_bin};

/// Compile `source` (Markdown) to `out_pdf` using the given typst binary.
/// The intermediate `.typ` is written beside the source so relative `image()`
/// paths resolve, and `--root` is pinned to the source's directory.
fn compile_pdf(typst: &Path, source: &Path, out_pdf: &Path, title: &str) -> Result<()> {
    let md = fs::read_to_string(source).with_context(|| format!("reading {}", source.display()))?;
    let doc = markdown::document(&md, Some(title));

    let dir = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "doc".into());
    let typ_path = dir.join(format!(".{stem}.byteferret.typ"));
    fs::write(&typ_path, doc).with_context(|| format!("writing {}", typ_path.display()))?;

    let out = Command::new(typst)
        .arg("compile")
        .arg("--root")
        .arg(dir)
        .arg(&typ_path)
        .arg(out_pdf)
        .output()
        .context("running `typst compile`")?;
    let _ = fs::remove_file(&typ_path); // best-effort cleanup of the scratch file
    if !out.status.success() {
        bail!(
            "typst compile failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Open a mail draft with the PDF attached via `xdg-email`.
#[cfg(not(target_os = "macos"))]
fn open_email(pdf: &Path, subject: &str) -> Result<()> {
    let status = Command::new("xdg-email")
        .arg("--subject")
        .arg(subject)
        .arg("--attach")
        .arg(pdf)
        .status()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow::anyhow!("`xdg-email` not found — install xdg-utils to use --email")
            }
            _ => anyhow::anyhow!("running xdg-email: {e}"),
        })?;
    if !status.success() {
        bail!("xdg-email exited with {status}");
    }
    Ok(())
}

/// macOS: `open -a Mail <file>` composes a new message with the file attached.
/// The subject can't be passed this way, so it is accepted-and-ignored here.
#[cfg(target_os = "macos")]
fn open_email(pdf: &Path, _subject: &str) -> Result<()> {
    let status = Command::new("open")
        .args(["-a", "Mail"])
        .arg(pdf)
        .status()
        .context("running `open -a Mail`")?;
    if !status.success() {
        bail!("open -a Mail exited with {status}");
    }
    Ok(())
}

pub fn publish(file: &str, _pdf: bool, out: Option<&str>, email: bool) -> Result<()> {
    let source = expand_path(file);
    if !source.is_file() {
        bail!("no such file: {}", source.display());
    }

    // PDF is the only supported output today; `--pdf` is accepted for forward
    // compatibility but rendering to PDF is the default regardless.
    let out_pdf: PathBuf = match out {
        Some(o) => expand_path(o),
        None => source.with_extension("pdf"),
    };
    let title = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Document".into());

    let paths = Paths::resolve();
    let typst = typst_bin::ensure_typst(&paths.typst_bin)?;
    compile_pdf(&typst, &source, &out_pdf, &title)?;

    say(&format!("Published → {}", out_pdf.display()));

    if email {
        open_email(&out_pdf, &title)?;
        say("Opened a mail draft with the PDF attached.");
    }

    emit(&json!({
        "ok": true,
        "source": source.to_string_lossy(),
        "pdf": out_pdf.to_string_lossy(),
        "emailed": email,
    }));
    Ok(())
}
