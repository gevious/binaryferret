#!/bin/sh
# ByteFerret LOCAL installer — build the agent from THIS checkout and install it.
#
# Unlike scripts/install.sh (which downloads a prebuilt release binary the way
# customers do), this script compiles the working tree with cargo and drops the
# resulting binary onto your PATH. Use it to test local changes end to end.
#
#   sh scripts/local-install.sh
#
# Environment overrides:
#   BYTEFERRET_PREFIX          install dir                 (default: ~/.local/bin)
#   BYTEFERRET_PROFILE         cargo profile: release|dev  (default: release)
#   BYTEFERRET_TARGET          override the build target triple
#                                (default: .cargo/config.toml's pin on Linux,
#                                 this host's Darwin triple on macOS)
#   BYTEFERRET_ENABLE_SERVICE  =1 to install+start the user service
set -eu

PREFIX="${BYTEFERRET_PREFIX:-$HOME/.local/bin}"
PROFILE="${BYTEFERRET_PROFILE:-release}"

log()  { printf '%s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Resolve the crate root from this script's location (scripts/ -> ..).
crate="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$crate/Cargo.toml" ] || die "cannot find Cargo.toml next to $crate — run this from the repo."

have cargo || die "cargo is required (install Rust: https://rustup.rs)."

# Assemble the cargo build flags.
set -- build
case "$PROFILE" in
  release) set -- "$@" --release ;;
  dev|debug) PROFILE=debug ;;
  *) die "BYTEFERRET_PROFILE must be 'release' or 'dev' (got '$PROFILE')." ;;
esac

target="${BYTEFERRET_TARGET:-}"
# .cargo/config.toml pins the Linux musl triple as the default target; on macOS
# override it with this host's Darwin triple so the build works out of the box.
if [ -z "$target" ] && [ "$(uname -s)" = "Darwin" ]; then
  case "$(uname -m)" in
    x86_64)        target="x86_64-apple-darwin" ;;
    arm64|aarch64) target="aarch64-apple-darwin" ;;
    *) die "unsupported architecture '$(uname -m)' (need x86_64 or arm64)." ;;
  esac
fi
[ -n "$target" ] && set -- "$@" --target "$target"

log "Building byteferret from source ($crate, profile=$PROFILE${target:+, target=$target})…"
( cd "$crate" && cargo "$@" )

# Locate the built binary. It lands in target/<profile>/ or, when a default
# target is pinned (.cargo/config.toml) or requested, target/<triple>/<profile>/.
built="$crate/target/$PROFILE/byteferret"
if [ ! -f "$built" ]; then
  built="$(find "$crate/target" -maxdepth 3 -type f -name byteferret -perm -u+x 2>/dev/null | head -n1)"
fi
[ -n "$built" ] && [ -f "$built" ] || die "build succeeded but the binary was not found under $crate/target."

mkdir -p "$PREFIX"
install -m 0755 "$built" "$PREFIX/byteferret"
log "Installed → $PREFIX/byteferret ($("$PREFIX/byteferret" --version 2>/dev/null || echo 'version unknown'))"

# PATH hint.
case ":$PATH:" in
  *":$PREFIX:"*) : ;;
  *) log ""
     log "Note: $PREFIX is not on your PATH. Add this to your shell profile:"
     log "  export PATH=\"$PREFIX:\$PATH\"" ;;
esac

if [ "${BYTEFERRET_ENABLE_SERVICE:-0}" = "1" ]; then
  log ""
  log "Setting up the user service (auto-start on login)…"
  "$PREFIX/byteferret" service install --now
else
  log ""
  log "Next steps:"
  log "  byteferret init ~/vault          # create a vault"
  log "  byteferret service install --now # auto-start on login (optional)"
  log "  byteferret pair --show           # begin pairing another machine"
fi
