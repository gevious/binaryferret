#!/usr/bin/env bash
#
# Regression e2e for folder isolation on accept (the "second folder overwrote my
# first" bug). Two fully isolated byteferret agents on one host, real binary.
#
# Proves two things about accepting a folder a peer offers, WITHOUT --path:
#   1. it lands in a NEW directory under the current working directory, and
#   2. two folders whose labels slugify to the SAME name never share a directory
#      — the second is auto-disambiguated instead of overwriting the first.
#
# Offline: reuses the Syncthing binary this machine already cached
# (~/.local/share/byteferret/bin/syncthing). Only a machine that has never run
# byteferret needs the network, and then just for the one pinned download.
#
# Usage: bash scripts/e2e-folder-isolation.sh   (builds the debug binary if needed)
#
set -euo pipefail
PROJ="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$PROJ/target/debug/byteferret"
[ -x "$BIN" ] || (cd "$PROJ" && cargo build)

ROOT="$(mktemp -d)"
trap 'echo "cleanup $ROOT"; run_a stop >/dev/null 2>&1 || true; run_b stop >/dev/null 2>&1 || true; rm -rf "$ROOT"' EXIT

# Distinct ports/dirs from e2e-local.sh so the two scripts can coexist.
run_a() { env BYTEFERRET_CONFIG_DIR="$ROOT/a/config" BYTEFERRET_DATA_DIR="$ROOT/a/data" \
  BYTEFERRET_GUI_ADDRESS=127.0.0.1:8411 BYTEFERRET_SYNC_ADDRESS=tcp://127.0.0.1:22020 \
  BYTEFERRET_DISCOVERY_PUBLIC=false "$BIN" "$@"; }
run_b() { env BYTEFERRET_CONFIG_DIR="$ROOT/b/config" BYTEFERRET_DATA_DIR="$ROOT/b/data" \
  BYTEFERRET_GUI_ADDRESS=127.0.0.1:8412 BYTEFERRET_SYNC_ADDRESS=tcp://127.0.0.1:22021 \
  BYTEFERRET_DISCOVERY_PUBLIC=false "$BIN" "$@"; }

jget() { python3 -c "import sys,json;print(json.load(sys.stdin)[\"$1\"])"; }

# Run agent B with a specific working directory — this is what an accept without
# --path keys off, so the test controls exactly where "the current dir" is.
run_b_in() { local dir="$1"; shift; ( cd "$dir" && run_b "$@" ); }

# Pre-seed the managed binary from this machine's cache so start() needs no net.
CACHED_BIN="${XDG_DATA_HOME:-$HOME/.local/share}/byteferret/bin/syncthing"
seed_bin() { if [ -x "$CACHED_BIN" ]; then mkdir -p "$1/bin"; cp "$CACHED_BIN" "$1/bin/syncthing"; fi; }
seed_bin "$ROOT/a/data"
seed_bin "$ROOT/b/data"

wait_for() { # $1 = path to appear, up to 60s
  for _ in $(seq 1 60); do [ -e "$1" ] && return 0; sleep 1; done; return 1; }

echo "== start both =="
run_a start; run_b start

echo "== init a folder on each, pair A<->B =="
run_a init "$ROOT/a/vault" >/dev/null   # folder name "vault"
run_b init "$ROOT/b/vault" >/dev/null   # folder name "vault" (distinct id — random suffix)
ID_A=$(run_a status --json | jget deviceId)
ID_B=$(run_b status --json | jget deviceId)
run_b pair --with "$ID_A" --address tcp://127.0.0.1:22020 >/dev/null   # B has one folder → shared by default
sleep 2
run_a pair "$ID_B" --accept --folder vault --address tcp://127.0.0.1:22021 >/dev/null

# Two DISTINCT folders on A (folder names are unique per machine now). Distinct
# contents so any cross-contamination between them would be visible.
echo "== A: two folders, shared with B =="
mkdir -p "$ROOT/a/d1" "$ROOT/a/d2"
echo "i-am-folder-one" > "$ROOT/a/d1/one.md"
echo "i-am-folder-two" > "$ROOT/a/d2/two.md"
FID1=$(run_a init "$ROOT/a/d1" --label "Alpha" --json | jget folderId)
FID2=$(run_a init "$ROOT/a/d2" --label "Beta"  --json | jget folderId)
echo "  folder 1: $FID1"
echo "  folder 2: $FID2"
[ "$FID1" != "$FID2" ] || { echo "FAIL: the two folders share an id"; exit 1; }

# A folder name is unique on a machine: a second folder whose name slugs to an
# existing one is refused, not silently turned into a suffixed lookalike.
echo "== A: a duplicate folder name is refused =="
mkdir -p "$ROOT/a/dupe"
if run_a init "$ROOT/a/dupe" --label "Alpha" >/dev/null 2>&1; then
  echo "FAIL: init allowed a duplicate folder name"; exit 1
fi
echo "  refused, as expected"

# Folders are shared and accepted by their visible name (the random id suffix is
# never typed).
run_a pair --with "$ID_B" --folder alpha >/dev/null
run_a pair --with "$ID_B" --folder beta  >/dev/null
sleep 2

# B accepts both from a chosen working directory, WITHOUT --path. Each must land
# in its own new directory under WORK, named after its (unique) folder name.
WORK="$ROOT/b/work"; mkdir -p "$WORK"
echo "== B: accept both without --path, from $WORK =="
OUT1=$(run_b_in "$WORK" pair "$ID_A" --accept --folder alpha --json)
OUT2=$(run_b_in "$WORK" pair "$ID_A" --accept --folder beta  --json)

DIR1="$WORK/alpha"
DIR2="$WORK/beta"
[ -d "$DIR1" ] || { echo "FAIL: folder 1 did not land at $DIR1"; echo "$OUT1"; exit 1; }
[ -d "$DIR2" ] || { echo "FAIL: folder 2 did not land at $DIR2"; echo "$OUT2"; exit 1; }
[ "$DIR2" != "$DIR1" ] || { echo "FAIL: both folders landed in the same directory"; exit 1; }
echo "  folder 1 -> $DIR1"
echo "  folder 2 -> $DIR2"

echo "== wait for both to sync =="
wait_for "$DIR1/one.md" || { echo "FAIL: folder 1 never synced"; exit 1; }
wait_for "$DIR2/two.md" || { echo "FAIL: folder 2 never synced"; exit 1; }
# Let a stray cross-sync (the bug) have time to show up before we assert purity.
sleep 3

echo "== assert no cross-contamination =="
grep -q i-am-folder-one "$DIR1/one.md" || { echo "FAIL: folder 1 content is wrong"; exit 1; }
grep -q i-am-folder-two "$DIR2/two.md" || { echo "FAIL: folder 2 content is wrong"; exit 1; }
[ ! -e "$DIR1/two.md" ] || { echo "FAIL: folder 2's file leaked into folder 1 ($DIR1/two.md)"; exit 1; }
[ ! -e "$DIR2/one.md" ] || { echo "FAIL: folder 1's file leaked into folder 2 ($DIR2/one.md)"; exit 1; }

echo "== assert accepting onto another folder's dir is refused =="
# A fresh third folder from A; accepting it with --path pointed at folder 1's
# directory must be rejected, not merged. (Must be a folder B does not already
# have, or accept would just re-share the existing one and ignore --path.)
mkdir -p "$ROOT/a/d3"; echo "i-am-folder-three" > "$ROOT/a/d3/three.md"
FID3=$(run_a init "$ROOT/a/d3" --label "Third" --json | jget folderId)
run_a pair --with "$ID_B" --folder "$FID3" >/dev/null
sleep 2
if run_b_in "$WORK" pair "$ID_A" --accept --folder "$FID3" --path "$DIR1" >/dev/null 2>&1; then
  echo "FAIL: accept was allowed onto an existing folder's directory"; exit 1
fi
[ ! -e "$DIR1/three.md" ] || { echo "FAIL: refused accept still leaked a file into folder 1"; exit 1; }
echo "  refused, as expected"

run_a stop >/dev/null; run_b stop >/dev/null
echo "PASS"
