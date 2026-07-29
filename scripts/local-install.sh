#!/bin/sh
# BinaryFerret LOCAL installer — build the agent from THIS checkout and install it.
#
# Unlike scripts/install.sh (which downloads a prebuilt release binary the way
# customers do), this script compiles the working tree with cargo and drops the
# resulting binary onto your PATH. Use it to test local changes end to end.
#
#   sh scripts/local-install.sh
#
# Environment overrides:
#   BINARYFERRET_PREFIX          install dir                 (default: ~/.local/bin)
#   BINARYFERRET_PROFILE         cargo profile: release|dev  (default: release)
#   BINARYFERRET_TARGET          override the build target triple
#                                (default: whatever .cargo/config.toml pins)
#   BINARYFERRET_ENABLE_SERVICE  =1 to install+start the systemd user service
set -eu

PREFIX="${BINARYFERRET_PREFIX:-$HOME/.local/bin}"
PROFILE="${BINARYFERRET_PROFILE:-release}"

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
  *) die "BINARYFERRET_PROFILE must be 'release' or 'dev' (got '$PROFILE')." ;;
esac

target="${BINARYFERRET_TARGET:-}"
[ -n "$target" ] && set -- "$@" --target "$target"

log "Building binaryferret from source ($crate, profile=$PROFILE${target:+, target=$target})…"
( cd "$crate" && cargo "$@" )

# Locate the built binary. It lands in target/<profile>/ or, when a default
# target is pinned (.cargo/config.toml) or requested, target/<triple>/<profile>/.
built="$crate/target/$PROFILE/binaryferret"
if [ ! -f "$built" ]; then
  built="$(find "$crate/target" -maxdepth 3 -type f -name binaryferret -perm -u+x 2>/dev/null | head -n1)"
fi
[ -n "$built" ] && [ -f "$built" ] || die "build succeeded but the binary was not found under $crate/target."

mkdir -p "$PREFIX"
install -m 0755 "$built" "$PREFIX/binaryferret"
log "Installed → $PREFIX/binaryferret ($("$PREFIX/binaryferret" --version 2>/dev/null || echo 'version unknown'))"

# PATH hint.
case ":$PATH:" in
  *":$PREFIX:"*) : ;;
  *) log ""
     log "Note: $PREFIX is not on your PATH. Add this to your shell profile:"
     log "  export PATH=\"$PREFIX:\$PATH\"" ;;
esac

if [ "${BINARYFERRET_ENABLE_SERVICE:-0}" = "1" ]; then
  log ""
  log "Setting up the systemd user service…"
  "$PREFIX/binaryferret" service install --now
else
  log ""
  log "Next steps:"
  log "  binaryferret init ~/vault          # create a vault"
  log "  binaryferret service install --now # auto-start on login (optional)"
  log "  binaryferret pair --show           # begin pairing another machine"
fi
