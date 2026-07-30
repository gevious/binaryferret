# ByteFerret

**Peer-to-peer document vault for your own machines.** ByteFerret keeps a folder
of Markdown notes (and their images, PDFs, and other assets) in sync across your
Linux and macOS machines — directly, machine-to-machine, with no account and no
server in the middle. It does this by bundling and orchestrating a private, version-pinned
[Syncthing](https://syncthing.net/); you drive everything through one small
`byteferret` CLI.

Ships as a single small binary: statically linked musl (~1.5 MB) on Linux, a
native Darwin build on macOS.

> **Status: Phase-1 MVP.** The peer-to-peer core (init a vault, pair machines,
> auto-sync) works today. The always-on hub, web UI, and mobile app are later
> phases. See the `backlog.md` and `getting-started.md` guides in the docs repo
> for the roadmap and what is / isn't implemented yet.

## Install

```sh
curl -fsSL https://get.byteferret.com/install.sh | sh
```

This detects your OS/arch (Linux or macOS, x86_64 or arm64) and downloads the
latest prebuilt binary to `~/.local/bin/byteferret`. Options via
environment variables (see [`scripts/install.sh`](scripts/install.sh)):

- `BYTEFERRET_ENABLE_SERVICE=1` — also install & start the user service
  (systemd on Linux, launchd on macOS).
- `BYTEFERRET_FROM_SOURCE=1` — build with `cargo` instead of downloading.
- `BYTEFERRET_PREFIX=/some/dir` — install somewhere other than `~/.local/bin`.

### From source

```sh
# Linux — produces a static musl binary (see .cargo/config.toml)
cargo build --release

# macOS — override the pinned musl default with this host's Darwin triple
cargo build --release --target aarch64-apple-darwin   # or x86_64-apple-darwin

# either OS: build the working tree and install it in one step
sh scripts/local-install.sh
```

On Linux this requires the `x86_64-unknown-linux-musl` (or `aarch64-…`) Rust
target and `musl-tools`. `curl` and `tar` must be present at runtime — used once
to fetch the pinned Syncthing (and Typst, for `publish`).

## Quick start (peer-to-peer)

```sh
byteferret init ~/vault            # create a vault (scaffolds a starter structure)
byteferret service install --now   # auto-start the agent on login (optional)

# On machine A:
byteferret status                  # prints this machine's device ID

# On machine B:
byteferret pair --with <A's-device-id>

# Back on machine A:
byteferret status                  # shows B's request, with its device ID
byteferret pair <B's-device-id> --accept                        # approve the machine
byteferret pair <B's-device-id> --accept --folder <folder-id>   # share a folder with it

byteferret status                  # health, device id, peers, folders, sync state
```

Edit anything in `~/vault` and it syncs to your paired machines automatically.

### Accepting is per machine *and* per folder

Approving a machine and giving it a folder are two separate steps. `--accept`
on its own admits the machine and shares nothing; each folder is then granted
individually, so one peer can be given some folders and not others:

```sh
byteferret status                                   # lists requests and offered folders
byteferret pair <id> --accept --folder notes --folder recipes   # grant two folders
byteferret pair <id> --reject --folder recipes      # withdraw one, keep the rest
byteferret pair <id> --accept --all-folders         # take up everything it offers
```

A folder a peer offers is created beside your vault, or wherever `--path` says.
Rejecting an offer declines it; rejecting a folder you already share stops
sharing it, leaving the folder and its files in place.

### Syncing more folders

Any directory — new or already full of files — can be brought into sync;
folder ids are generated automatically (the first `init` is special: it becomes
the vault). One folder can be shared with any number of peers, and each peer
can get a different subset:

```sh
byteferret init ~/recipes --label Recipes        # existing dir, nothing changed in it
byteferret pair --with <peer-id> --folder <id>   # share it (id shown by init/status)
```

Both verbs always name one peer by device ID — there is no "accept everything
waiting". Anyone who can reach your machine can put a request in that list, so a
bulk accept would hand them a folder alongside the machine you meant to approve.
Peer-supplied *names* are shown but never matched, and an ambiguous device-ID
prefix is refused rather than guessed.
A fuller two-machine walkthrough (`RUN-TWO-DESKTOPS.md`) and the product vision
+ Path B / hub (`getting-started.md`) live in the docs repo.

## Commands

| Command | What it does |
|---|---|
| `byteferret start` / `stop` | Start/stop the managed Syncthing (idempotent) |
| `byteferret init <path> [--label <name>]` | Bring a folder into sync (first becomes the vault; ids automatic) |
| `byteferret pair --with <id> [--folder <f>]` | Pair with a machine and share a folder |
| `byteferret pair <id> --accept \| --reject [--folder <f> \| --all-folders]` | Approve/decline a machine, or one of its folders |
| `byteferret status` | Agent health, hostname + device ID, peers & folders, requests, conflicts |
| `byteferret doctor [--fix]` | Diagnose (and optionally repair) the setup |
| `byteferret logs [-n N] [-f]` | Show/follow the agent log |
| `byteferret publish <file> [--pdf] [--email]` | Render a doc to PDF locally |
| `byteferret service install \| uninstall \| status` | Manage the user service (systemd/launchd) |
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
| `.cargo/config.toml` | Pins the default (Linux musl) build target. |
| `.agent/CONTEXT.md` | Architecture/domain context for the codebase. |

## License

[AGPL-3.0-only](LICENSE). Syncthing is bundled as a separate MPL-2.0 dependency.
