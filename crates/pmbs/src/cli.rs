//! PMBS CLI Definitions powered by Clap 4

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 8007;
pub const DEFAULT_DATA_DIR: &str = "/var/lib/pmbs";
pub const DEFAULT_SOCKET: &str = "/var/lib/pmbs/pmbs.sock";

#[derive(Parser, Debug)]
#[command(
    name = "pmbs",
    author = "Xavier DP <xavier@xavdp.pro>",
    version = "2.1.0",
    about = "PodMesh Backup Server (PMBS) — Enterprise Deduplicated Backup Server in Pure Rust",
    long_about = "High-performance PBS-alternative backup server for PodMesh and sovereign servers.\nFeatures 4MB SHA-256 CAS chunk deduplication, Op 10 Sandbox Boot Proof verification, and inter-site sync."
)]
pub struct Cli {
    /// Enable verbose debug logging
    #[arg(short, long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the PMBS backup daemon and REST API
    Daemon(DaemonArgs),

    /// Display PMBS server status and datastores
    Status(StatusArgs),

    /// Quick ping health check via Unix Socket or HTTP
    Ping(PingArgs),

    /// Datastore management commands
    Datastore(DatastoreCommand),

    /// Verify backup snapshots and Op 10 Sandbox Boot Proof
    Verify(VerifyArgs),

    /// Trigger inter-site differential replication
    Sync(SyncArgs),
}

#[derive(Args, Debug)]
pub struct DaemonArgs {
    /// Port to listen on for REST API
    #[arg(short, long, default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// Root directory for PMBS datastores
    #[arg(short, long, default_value = DEFAULT_DATA_DIR)]
    pub data_dir: PathBuf,

    /// Path to Unix Domain Socket for local IPC control
    #[arg(short, long, default_value = DEFAULT_SOCKET)]
    pub socket: PathBuf,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// PMBS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:8007")]
    pub remote: String,

    /// Unix Domain Socket path to query directly
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PingArgs {
    /// PMBS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:8007")]
    pub remote: String,

    /// Unix Domain Socket path to query directly
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct DatastoreCommand {
    #[command(subcommand)]
    pub action: DatastoreSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum DatastoreSubcommand {
    /// List all datastores
    List {
        #[arg(short, long, default_value = "http://127.0.0.1:8007")]
        remote: String,
    },
    /// Create a new datastore
    Create {
        /// Name of the datastore
        name: String,
        #[arg(short, long, default_value = "http://127.0.0.1:8007")]
        remote: String,
    },
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// Datastore name
    #[arg(short, long, default_value = "datastore-default")]
    pub datastore: String,

    /// Specific snapshot ID to verify (all if omitted)
    #[arg(short, long)]
    pub snapshot: Option<String>,

    /// PMBS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:8007")]
    pub remote: String,

    /// Unix Domain Socket path to query directly
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Remote PMBS base URL (e.g. http://192.168.1.100:8007)
    pub remote_url: String,

    /// Remote datastore name to sync from
    #[arg(short, long, default_value = "datastore-default")]
    pub remote_datastore: String,

    /// Local datastore to store replicated chunks
    #[arg(short, long, default_value = "datastore-default")]
    pub target_datastore: String,

    /// Local PMBS HTTP address
    #[arg(short, long, default_value = "http://127.0.0.1:8007")]
    pub remote: String,
}
