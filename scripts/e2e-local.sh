#!/usr/bin/env bash
#
# End-to-end P2P test: two fully isolated binaryferret agents on one host.
# Proves start -> init -> pair -> bidirectional sync using the real binary.
#
# Usage: bash scripts/e2e-local.sh   (builds the release binary if needed)
#
set -euo pipefail
PROJ="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$PROJ/target/x86_64-unknown-linux-musl/release/binaryferret"
[ -x "$BIN" ] || (cd "$PROJ" && cargo build --release)

ROOT="$(mktemp -d)"
trap 'echo "cleanup $ROOT"; rm -rf "$ROOT"' EXIT

run_a() { env BINARYFERRET_CONFIG_DIR="$ROOT/a/config" BINARYFERRET_DATA_DIR="$ROOT/a/data" \
  BINARYFERRET_GUI_ADDRESS=127.0.0.1:8401 BINARYFERRET_SYNC_ADDRESS=tcp://127.0.0.1:22010 \
  BINARYFERRET_DISCOVERY_PUBLIC=false "$BIN" "$@"; }
run_b() { env BINARYFERRET_CONFIG_DIR="$ROOT/b/config" BINARYFERRET_DATA_DIR="$ROOT/b/data" \
  BINARYFERRET_GUI_ADDRESS=127.0.0.1:8402 BINARYFERRET_SYNC_ADDRESS=tcp://127.0.0.1:22011 \
  BINARYFERRET_DISCOVERY_PUBLIC=false "$BIN" "$@"; }

echo "== start both =="; run_a start; run_b start
echo "== init vaults =="; run_a init "$ROOT/a/vault"; run_b init "$ROOT/b/vault"

echo "== pair =="
ID_A=$(run_a pair --show --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["deviceId"])')
run_b pair --with "$ID_A" --address tcp://127.0.0.1:22010
sleep 2
run_a pair --accept --address tcp://127.0.0.1:22011

echo "== sync A->B =="
echo "# hello $(date)" > "$ROOT/a/vault/hello.md"
ok=no
for i in $(seq 1 60); do [ -f "$ROOT/b/vault/hello.md" ] && { echo "synced A->B in ${i}s"; ok=yes; break; }; sleep 1; done

echo "== sync B->A =="
echo "reply" >> "$ROOT/b/vault/hello.md"
for i in $(seq 1 60); do grep -q reply "$ROOT/a/vault/hello.md" 2>/dev/null && { echo "synced B->A in ${i}s"; break; }; sleep 1; done

run_a status
run_a stop; run_b stop
[ "$ok" = yes ] || { echo "FAIL"; exit 1; }
echo "PASS"
