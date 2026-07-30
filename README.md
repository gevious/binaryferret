# ByteFerret

**Peer-to-peer document vault for your own machines.** ByteFerret keeps a folder
of Markdown notes (and their images, PDFs, and other assets) in sync across your
Linux desktops — directly, machine-to-machine, with no account and no server in
the middle. It does this by bundling and orchestrating a private, version-pinned
[Syncthing](https://syncthing.net/); you drive everything through one small
`byteferret` CLI.

Ships as a single, statically linked (~1.5 MB) musl binary.

> **Status: Phase-1 MVP.** The peer-to-peer core (init a vault, pair machines,
> auto-sync) works today. The always-on hub, web UI, and mobile app are later
> phases. See the `backlog.md` and `getting-started.md` guides in the docs repo
> for the roadmap and what is / isn't implemented yet.

## Install

```sh
curl -fsSL https://byteferret.com/install.sh | sh
```

This downloads the prebuilt binary to `~/.local/bin/byteferret`. Options via
environment variables (see [`scripts/install.sh`](scripts/install.sh)):

- `BYTEFERRET_ENABLE_SERVICE=1` — also install & start the systemd user service.
- `BYTEFERRET_FROM_SOURCE=1` — build with `cargo` instead of downloading.
- `BYTEFERRET_PREFIX=/some/dir` — install somewhere other than `~/.local/bin`.

### From source

```sh
cargo build --release   # produces a static musl binary (see .cargo/config.toml)
```

Requires the `x86_64-unknown-linux-musl` (or `aarch64-…`) Rust target and
`musl-tools`. `curl` and `tar` must be present at runtime — used once to fetch
the pinned Syncthing (and Typst, for `publish`).

## Quick start (peer-to-peer)

```sh
byteferret init ~/vault            # create a vault (scaffolds a starter structure)
byteferret service install --now   # auto-start the agent on login (optional)

# On machine A:
byteferret pair --show             # prints this machine's device ID

# On machine B:
byteferret pair --with <A's-device-id>

# Back on machine A:
byteferret pair --accept           # approve the request

byteferret status                  # peers, sync state, conflicts
```

Edit anything in `~/vault` and it syncs to your paired machines automatically.
A fuller two-machine walkthrough (`RUN-TWO-DESKTOPS.md`) and the product vision
+ Path B / hub (`getting-started.md`) live in the docs repo.

## Commands

| Command | What it does |
|---|---|
| `byteferret start` / `stop` | Start/stop the managed Syncthing (idempotent) |
| `byteferret init <path> [--existing]` | Create or attach a vault |
| `byteferret pair --show \| --with <id> \| --accept` | Peer-to-peer pairing |
| `byteferret status` | Agent health, peers, sync state, conflicts |
| `byteferret doctor [--fix]` | Diagnose (and optionally repair) the setup |
| `byteferret logs [-n N] [-f]` | Show/follow the agent log |
| `byteferret publish <file> [--pdf] [--email]` | Render a doc to PDF locally |
| `byteferret service install \| uninstall \| status` | Manage the systemd user service |
| `byteferret version` | Agent + pinned Syncthing versions |

Every command accepts `--json` for machine-readable output.

## How it works

The agent never edits Syncthing's config files directly — it downloads a pinned
Syncthing, generates its keys, runs it detached bound to localhost with a random
API key (`0600`), and drives it entirely over the localhost REST API. Config
lives in `~/.config/byteferret/` (`config.toml` + a `0600` `secrets` file); managed
state in `~/.local/share/byteferret/`. See `.agent/CONTEXT.md` for the architecture
(and `DESIGN.md` in the docs repo for the full design).

## Repository layout

| Path | What |
|---|---|
| `src/` | Rust sources (`main.rs`, `commands/`, `syncthing/`, `publish/`, …). |
| `scripts/` | `install.sh` (curl-installer) and `e2e-local.sh` (two-agent P2P test). |
| `.cargo/config.toml` | Pins the default musl build target. |
| `.agent/CONTEXT.md` | Architecture/domain context for the codebase. |

## License

[AGPL-3.0-only](LICENSE). Syncthing is bundled as a separate MPL-2.0 dependency.
