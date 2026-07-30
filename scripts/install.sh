#!/bin/sh
# ByteFerret installer — fetch the prebuilt agent binary and (optionally) set up
# the systemd user service. Designed to be run straight from a URL:
#
#   curl -fsSL https://byteferret.com/install.sh | sh
#
# Environment overrides:
#   BYTEFERRET_REPO            GitHub owner/repo         (default: gevious/byteferret)
#   BYTEFERRET_VERSION         release tag to install    (default: latest)
#   BYTEFERRET_PREFIX          install dir               (default: ~/.local/bin)
#   BYTEFERRET_ENABLE_SERVICE  =1 to install+start the systemd user service
#   BYTEFERRET_FROM_SOURCE     =1 to build with cargo instead of downloading
set -eu

REPO="${BYTEFERRET_REPO:-gevious/byteferret}"
PREFIX="${BYTEFERRET_PREFIX:-$HOME/.local/bin}"
VERSION="${BYTEFERRET_VERSION:-latest}"

log()  { printf '%s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

detect_target() {
  os="$(uname -s)"
  [ "$os" = "Linux" ] || die "ByteFerret v1 supports Linux only (found $os)."
  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64)  echo "x86_64-unknown-linux-musl" ;;
    aarch64|arm64) echo "aarch64-unknown-linux-musl" ;;
    *) die "unsupported architecture '$arch' (need x86_64 or arm64)." ;;
  esac
}

# Resolve "latest" to a concrete tag via the GitHub API.
resolve_version() {
  [ "$VERSION" != "latest" ] && { echo "$VERSION"; return; }
  api="https://api.github.com/repos/$REPO/releases/latest"
  tag="$(curl -fsSL "$api" 2>/dev/null | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$tag" ] || die "could not determine the latest release of $REPO — set BYTEFERRET_VERSION or use BYTEFERRET_FROM_SOURCE=1."
  echo "$tag"
}

install_binary() {
  have curl || die "curl is required."
  have tar  || die "tar is required."
  target="$(detect_target)"
  tag="$(resolve_version)"
  asset="byteferret-$target.tar.gz"
  url="https://github.com/$REPO/releases/download/$tag/$asset"

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  log "Downloading byteferret $tag ($target)…"
  curl -fSL --proto '=https' -o "$tmp/$asset" "$url" \
    || die "download failed: $url"
  tar -xzf "$tmp/$asset" -C "$tmp" || die "extract failed for $asset"
  [ -f "$tmp/byteferret" ] || die "archive did not contain a 'byteferret' binary."

  mkdir -p "$PREFIX"
  install -m 0755 "$tmp/byteferret" "$PREFIX/byteferret"
  log "Installed → $PREFIX/byteferret"
}

build_from_source() {
  have cargo || die "BYTEFERRET_FROM_SOURCE=1 needs cargo (install Rust: https://rustup.rs)."
  # Locate the crate root (the dir holding Cargo.toml). This script lives at
  # agent/scripts/install.sh, so prefer its own ../; otherwise fall back to a
  # checkout's agent/ or the current directory.
  crate=""
  if [ -n "${0:-}" ] && [ -f "$(dirname "$0")/../Cargo.toml" ]; then
    crate="$(cd "$(dirname "$0")/.." && pwd)"
  elif [ -f "agent/Cargo.toml" ]; then crate="agent"
  elif [ -f "Cargo.toml" ]; then crate="."
  else die "BYTEFERRET_FROM_SOURCE=1 must run inside a checkout (no Cargo.toml found)."
  fi
  log "Building byteferret from source ($crate)…"
  ( cd "$crate" && cargo build --release )
  # The binary lands in target/release/ or, when .cargo/config.toml pins a
  # default target (e.g. the musl triple), target/<triple>/release/.
  built="$crate/target/release/byteferret"
  [ -f "$built" ] || built="$(find "$crate/target" -maxdepth 3 -type f -name byteferret -perm -u+x 2>/dev/null | head -n1)"
  [ -n "$built" ] && [ -f "$built" ] || die "build succeeded but the binary was not found."
  mkdir -p "$PREFIX"
  install -m 0755 "$built" "$PREFIX/byteferret"
  log "Installed → $PREFIX/byteferret"
}

main() {
  if [ "${BYTEFERRET_FROM_SOURCE:-0}" = "1" ]; then
    build_from_source
  else
    install_binary
  fi

  # PATH hint.
  case ":$PATH:" in
    *":$PREFIX:"*) : ;;
    *) log ""
       log "Note: $PREFIX is not on your PATH. Add this to your shell profile:"
       log "  export PATH=\"$PREFIX:\$PATH\"" ;;
  esac

  if [ "${BYTEFERRET_ENABLE_SERVICE:-0}" = "1" ]; then
    log ""
    log "Setting up the systemd user service…"
    "$PREFIX/byteferret" service install --now
  else
    log ""
    log "Next steps:"
    log "  byteferret init ~/vault          # create a vault"
    log "  byteferret service install --now # auto-start on login (optional)"
    log "  byteferret pair --show           # begin pairing another machine"
  fi
}

main "$@"
