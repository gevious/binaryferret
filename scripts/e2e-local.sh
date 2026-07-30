#!/usr/bin/env bash
#
# End-to-end P2P test: two fully isolated byteferret agents on one host.
# Proves start -> init -> pair -> bidirectional sync using the real binary.
#
# Usage: bash scripts/e2e-local.sh   (builds the release binary if needed)
#
set -euo pipefail
PROJ="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$PROJ/target/x86_64-unknown-linux-musl/release/byteferret"
[ -x "$BIN" ] || (cd "$PROJ" && cargo build --release)

ROOT="$(mktemp -d)"
trap 'echo "cleanup $ROOT"; rm -rf "$ROOT"' EXIT

run_a() { env BYTEFERRET_CONFIG_DIR="$ROOT/a/config" BYTEFERRET_DATA_DIR="$ROOT/a/data" \
  BYTEFERRET_GUI_ADDRESS=127.0.0.1:8401 BYTEFERRET_SYNC_ADDRESS=tcp://127.0.0.1:22010 \
  BYTEFERRET_DISCOVERY_PUBLIC=false "$BIN" "$@"; }
run_b() { env BYTEFERRET_CONFIG_DIR="$ROOT/b/config" BYTEFERRET_DATA_DIR="$ROOT/b/data" \
  BYTEFERRET_GUI_ADDRESS=127.0.0.1:8402 BYTEFERRET_SYNC_ADDRESS=tcp://127.0.0.1:22011 \
  BYTEFERRET_DISCOVERY_PUBLIC=false "$BIN" "$@"; }

echo "== start both =="; run_a start; run_b start
echo "== init vaults =="; run_a init "$ROOT/a/vault"; run_b init "$ROOT/b/vault"

echo "== pair =="
ID_A=$(run_a status --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["deviceId"])')
ID_B=$(run_b status --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["deviceId"])')
run_b pair --with "$ID_A" --address tcp://127.0.0.1:22010
sleep 2
# Accept B and share the vault back (accepting alone admits the machine only).
run_a pair "$ID_B" --accept --folder byteferret-vault --address tcp://127.0.0.1:22011

echo "== sync A->B =="
echo "# hello $(date)" > "$ROOT/a/vault/hello.md"
ok=no
for i in $(seq 1 60); do [ -f "$ROOT/b/vault/hello.md" ] && { echo "synced A->B in ${i}s"; ok=yes; break; }; sleep 1; done

echo "== sync B->A =="
echo "reply" >> "$ROOT/b/vault/hello.md"
for i in $(seq 1 60); do grep -q reply "$ROOT/a/vault/hello.md" 2>/dev/null && { echo "synced B->A in ${i}s"; break; }; sleep 1; done

echo "== second folder (existing dir, auto id) =="
mkdir -p "$ROOT/a/recipes"
echo "pancakes" > "$ROOT/a/recipes/r1.md"
FID=$(run_a init "$ROOT/a/recipes" --label Recipes --json | python3 -c 'import sys,json;print(json.load(sys.stdin)["folderId"])')
echo "generated folder id: $FID"
run_a pair --with "$ID_B" --folder "$FID"
sleep 2
run_b pair "$ID_A" --accept --folder "$FID" --path "$ROOT/b/recipes"
ok2=no
for i in $(seq 1 60); do [ -f "$ROOT/b/recipes/r1.md" ] && { echo "second folder synced in ${i}s"; ok2=yes; break; }; sleep 1; done

run_a status
run_a stop; run_b stop
[ "$ok" = yes ] || { echo "FAIL: vault did not sync"; exit 1; }
[ "$ok2" = yes ] || { echo "FAIL: second folder did not sync"; exit 1; }
echo "PASS"
