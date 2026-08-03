# ByteFerret agent — CONTEXT

Domain map for the desktop agent (Phase 1 MVP vertical slice). See `docs/requirements.md`
for the full FR spec and `docs/backlog.md` for phasing.

## What this is
A **Rust** CLI (`byteferret`) that orchestrates a **bundled, version-pinned Syncthing**
(v`1.30.0`) to sync a document vault **peer-to-peer** across a user's own Linux
machines. No hub, no account (Path A of `docs/getting-started.md`). Ships as a single
statically linked **musl** binary (~1.4 MB); default build target is
`x86_64-unknown-linux-musl` (see `agent/.cargo/config.toml`).

The Rust crate lives in **`agent/`** (`agent/Cargo.toml`, `agent/src/`,
`agent/scripts/`); the marketing/brochure site lives in **`website/`** (SvelteKit).
Top-level `*.md` files are project docs. Paths below are relative to `agent/`.

## Key terms
- **Vault** — a user folder of Markdown + `assets/`, registered with Syncthing as a
  *folder*. All machines' default vaults share one folder id (`byteferret-vault`) so
  pairing links them.
- **Device ID** — a machine's Syncthing cryptographic identity; the pairing token.
- **Peer** — another machine running the agent, added as a Syncthing *device* and
  granted membership in the vault folder.
- **Managed Syncthing** — a private Syncthing instance the agent downloads, generates
  keys for, and runs as a detached background process (localhost REST only).

## Data flow (pair → sync)
`pair --show` prints device id → other machine `pair --with <id>` (adds device +
shares a folder) → first machine `pair <id> --accept` (approves the *connection*)
→ `pair <id> --accept --folder <f>` (grants one folder) → Syncthing connects both
directions and converges. Current-state sync is automatic and continuous; the
agent never runs a sync loop of its own.

Pairing is deliberately two decisions: admitting a machine grants access to no
folders, and each folder is then accepted or rejected per peer (`--folder`,
repeatable, or `--all-folders`). Both verbs require an explicit device-id target
and match on the id only — never on the peer-supplied name — refusing ambiguous
prefixes instead of guessing, so a stranger in the pending list cannot be swept
in alongside the machine the user meant.

## Module map (`src/`)
- `main.rs` — clap CLI definition + dispatch + top-level error handling.
- `commands/` — one file per command (`start stop init pair status version doctor logs
  publish service`); thin, express intent, print human + `--json`.
- `agent.rs` — orchestration facade: `ensure_started()` (start + persist), the
  device/folder primitives `add_device()` / `share_folder()` / `unshare_folder()` /
  `create_shared_folder()`, `resolve_device_target()` (id-only, ambiguity-refusing
  target matching), `vault_folder_config()`, runtime toggles.
- `fetch.rs` — `download_verified()`: curl + pinned-SHA-256 check before anything is
  extracted or run. The per-asset hashes live beside each downloader.
- `service.rs` — systemd user-unit *content* generation (pure, unit-tested);
  `commands/service.rs` owns all `systemctl --user` interaction.
- `publish/markdown.rs` — pure, unit-tested Markdown→Typst converter (vault subset).
  `publish/typst_bin.rs` — download/locate the pinned Typst per-arch (curl + system
  `tar -J` for the `.tar.xz` release). `commands/publish.rs` compiles + optional email.
- `syncthing/binary.rs` — locate/download the pinned Syncthing per-arch (via `curl`;
  extracts the exact top-level `syncthing` from the tarball).
- `syncthing/process.rs` — generate config, serve detached (`setsid` + pidfile), stop,
  health-poll.
- `syncthing/rest.rs` — `ureq` client over Syncthing's localhost REST API (HTTP, no
  TLS). **All** Syncthing interaction goes through here; config objects round-trip as
  `serde_json::Value` to preserve Syncthing's own fields.
- `paths.rs` — resolve XDG paths + `BYTEFERRET_*` overrides + pinned constants.
- `config.rs` — `config.toml` (shareable) and `secrets` (0600) load/save.
- `output.rs` — human/json output (global json flag) + `sanitize()`, which every
  peer-supplied string (device names, folder labels/ids, conflict filenames) passes
  through before reaching a terminal. `fsutil.rs` — path expand, conflict-file scan,
  and `safe_dir_name()` for reducing a peer's folder label to one safe component.

## Why these deps / build choices
- **ureq without TLS**: Syncthing REST is plain HTTP over localhost, so no TLS stack
  is linked. The one HTTPS fetch (the Syncthing tarball) uses system `curl`, keeping
  the static musl build free of ring/OpenSSL. `curl` is a first-run-only dependency.
  Because the agent never sees that TLS session, the download's pinned SHA-256 —
  not the transport — is what establishes the bytes are what upstream published.
- **clap** (CLI), **serde/serde_json** (REST + JSON out), **toml** (config),
  **flate2 + tar** (extract), **libc** (setsid/kill/urandom), **anyhow** (errors),
  **sha2** (verify downloads; pure Rust, no C dep, so musl is unaffected).

## On-disk layout
- `~/.config/byteferret/config.toml` — gui address, vault path, folder id (editable).
- `~/.config/byteferret/secrets` — Syncthing REST API key, `0600`.
- `~/.local/share/byteferret/` — `bin/syncthing`, `bin/typst`, `syncthing/` (its home),
  pidfile, log.
- `~/.config/systemd/user/byteferret.service` — the user unit (follows XDG_CONFIG_HOME,
  *not* BYTEFERRET_CONFIG_DIR; systemd only reads the standard location).

## Environment overrides (also enable local multi-instance testing)
- `BYTEFERRET_CONFIG_DIR`, `BYTEFERRET_DATA_DIR`, `BYTEFERRET_GUI_ADDRESS` — isolate an
  instance.
- `BYTEFERRET_SYNC_ADDRESS` — pin Syncthing's sync listen address.
- `BYTEFERRET_DISCOVERY_PUBLIC=false` — the getting-started `discovery.public` off
  switch (no global/local announce, no relays).

`agent/scripts/e2e-local.sh` runs two isolated agents on one host and asserts
bidirectional sync end-to-end.

## Status vs. spec (MVP slice)
Implemented: FR-1 (systemd user service via `service install`), FR-2/3 (bundled pinned
Syncthing, single static binary), FR-4 (idempotent), FR-5 (localhost REST + random
0600 key), FR-6 (fsWatcher + 60s rescan), FR-7 (no auto-accept), FR-8 (REST only),
FR-9/10 (init + scaffold + `.stignore`), FR-13 (manual pairing), FR-14 (auto sync),
FR-16 (conflict surfacing), FR-17 (minimal versioning), FR-19/20 (local `publish
--pdf`/`--email` via bundled Typst), FR-23/24/26/27/29 (CLI, `--json`, config,
secrets, status, `logs`, `doctor`). Distribution: `agent/scripts/install.sh` + GitHub
Actions CI (build/test/clippy) and a tag-triggered release workflow (x86_64 + arm64
musl). See `docs/ACCEPTANCE.md` for the Path-A checklist.
**Note:** `requirements.md §2` documents a TS/Bun stack; the implementation is Rust
per the owner's decision (smaller, dependency-free distributable binary).
**Deferred** (later phases): `byteferret config set`, short pairing code, gap-free
versioning, neovim `:ByteFerretPublish`/`:ByteFerretShare`, `enroll` + all hub/Path-B
features, offline-catchup tests, a public tagged release + the hosted installer
(live at `byteferret.com/install.sh`, served from the byteferret-website repo).
