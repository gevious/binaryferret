//! byteferret — P2P document vault agent (Path A, peer-to-peer).
//! Orchestrates a bundled, version-pinned Syncthing to sync a vault directly
//! between a user's own machines. See RUN-TWO-DESKTOPS.md.

mod agent;
mod commands;
mod config;
mod fetch;
mod fsutil;
mod output;
mod paths;
mod publish;
mod service;
mod syncthing;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "byteferret",
    version,
    about = "P2P document vault agent (peer-to-peer sync via Syncthing)"
)]
struct Cli {
    /// Machine-readable output (one JSON object on stdout)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the agent (downloads & runs Syncthing)
    Start,
    /// Stop the agent
    Stop,
    /// Create or attach a vault at <path>
    Init {
        path: String,
        /// Attach an existing folder instead of scaffolding (won't overwrite files)
        #[arg(long)]
        existing: bool,
    },
    /// Pair with another machine (Path A, peer-to-peer)
    Pair {
        /// Print this device's ID (to give to another machine)
        #[arg(long)]
        show: bool,
        /// Pair with a machine by its device ID and share a folder with it
        #[arg(long, value_name = "DEVICE-ID")]
        with: Option<String>,
        /// Approve the named peer's connection, or a folder of theirs with --folder
        #[arg(long)]
        accept: bool,
        /// Decline the named peer's request, or one folder with --folder
        #[arg(long)]
        reject: bool,
        /// Folder id to accept/reject/share; repeat for several
        #[arg(long, value_name = "FOLDER-ID")]
        folder: Vec<String>,
        /// Act on every folder the peer currently offers
        #[arg(long)]
        all_folders: bool,
        /// Where to put a folder taken up from a peer (defaults to beside the vault)
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
        /// Explicit peer sync address, e.g. tcp://host:22000 (manual/overlay pairing)
        #[arg(long)]
        address: Option<String>,
        /// Friendly name to record for the peer
        #[arg(long)]
        name: Option<String>,
        /// Device id (or an unambiguous prefix) that --accept/--reject applies to
        #[arg(value_name = "DEVICE-ID")]
        id: Option<String>,
    },
    /// Show agent, peers, sync state, conflicts
    Status,
    /// Print agent and Syncthing versions
    Version,
    /// Diagnose agent health (optionally repair safe issues)
    Doctor {
        /// Apply safe fixes (tighten secrets perms, start a stopped agent)
        #[arg(long)]
        fix: bool,
    },
    /// Show the agent's Syncthing log
    Logs {
        /// Number of trailing lines to show
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Follow the log, printing new lines as they arrive
        #[arg(short = 'f', long)]
        follow: bool,
        /// Print the log file path and exit
        #[arg(long)]
        path: bool,
    },
    /// Render a vault document to PDF (and optionally email it)
    Publish {
        /// Path to the Markdown document to render
        file: String,
        /// Render to PDF (the default and only format today)
        #[arg(long)]
        pdf: bool,
        /// Output path (defaults to the source with a .pdf extension)
        #[arg(long)]
        out: Option<String>,
        /// Open a mail draft with the PDF attached (via xdg-email)
        #[arg(long)]
        email: bool,
    },
    /// Manage the systemd user service (auto-start on login)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    /// Install and enable the user service
    Install {
        /// Start it immediately as well as enabling it on login
        #[arg(long)]
        now: bool,
    },
    /// Stop, disable, and remove the user service
    Uninstall,
    /// Show whether the service is installed, enabled, and active
    Status,
}

fn main() {
    let cli = Cli::parse();
    output::set_json_mode(cli.json);

    let result = match &cli.cmd {
        Cmd::Start => commands::start::start(),
        Cmd::Stop => commands::stop::stop(),
        Cmd::Init { path, existing } => commands::init::init(path, *existing),
        Cmd::Pair {
            show,
            with,
            accept,
            reject,
            folder,
            all_folders,
            path,
            address,
            name,
            id,
        } => commands::pair::pair(commands::pair::PairArgs {
            show: *show,
            with: with.as_deref(),
            accept: *accept,
            reject: *reject,
            target: id.as_deref(),
            address: address.as_deref(),
            name: name.as_deref(),
            folders: folder.clone(),
            all_folders: *all_folders,
            path: path.as_deref(),
        }),
        Cmd::Status => commands::status::status(),
        Cmd::Version => commands::version::version(),
        Cmd::Doctor { fix } => commands::doctor::doctor(*fix),
        Cmd::Logs {
            lines,
            follow,
            path,
        } => commands::logs::logs(*lines, *follow, *path),
        Cmd::Publish {
            file,
            pdf,
            out,
            email,
        } => commands::publish::publish(file, *pdf, out.as_deref(), *email),
        Cmd::Service { action } => match action {
            ServiceAction::Install { now } => commands::service::install(*now),
            ServiceAction::Uninstall => commands::service::uninstall(),
            ServiceAction::Status => commands::service::status(),
        },
    };

    if let Err(e) = result {
        if output::is_json_mode() {
            println!(
                "{}",
                serde_json::json!({ "ok": false, "error": e.to_string() })
            );
        } else {
            eprintln!("\nerror: {e:#}");
        }
        std::process::exit(1);
    }
}
