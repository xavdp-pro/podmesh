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
    version = "2.4.1",
    about = "PodMesh Backup Server (PMBS) — Enterprise Deduplicated Backup Server in Pure Rust",
    long_about = "High-performance PBS-alternative backup server for PodMesh and sovereign servers.\nFeatures 4MB BLAKE3 CAS chunk deduplication, referenced-chunk integrity verification (no boot proof), and inter-site sync."
)]
pub struct Cli {
    /// Enable verbose debug logging
    #[arg(long, global = true)]
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

    /// Audit referenced backup chunks (does not restore or boot workloads)
    Verify(VerifyArgs),

    /// Restore regular file contents from authenticated server snapshot metadata.
    /// Does not execute workloads or restore permissions, ACLs, xattrs or symlinks.
    Restore(RestoreArgs),

    /// Capture regular contents of an explicitly quiesced directory; create-only snapshot.
    Backup(BackupArgs),

    /// Trigger inter-site differential replication
    Sync(SyncArgs),
}

#[derive(Args, Debug)]
pub struct DaemonArgs {
    /// Listen address; remote access requires an authenticated TLS boundary.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: std::net::IpAddr,
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

#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// New destination directory; it and its symlink form must not exist.
    pub destination: PathBuf,
    #[arg(long, default_value = "datastore-default")]
    pub datastore: String,
    #[arg(long)]
    pub snapshot: String,
    /// Required along with snapshot ID to disambiguate server snapshot identity.
    #[arg(long)]
    pub backup_type: String,
    /// HTTPS endpoint, or numeric loopback HTTP. Uses PMBS_API_TOKEN, no redirects.
    #[arg(long, default_value = "http://127.0.0.1:8007")]
    pub remote: String,
    /// Maximum logical restored bytes (default 10 GiB); checked before writing.
    #[arg(long, default_value_t = 10 * 1024 * 1024 * 1024u64)]
    pub max_bytes: u64,
}

#[derive(Args, Debug)]
pub struct BackupArgs {
    /// Trusted source directory; symlinks in any ancestor or entry are refused.
    /// Only logical relative names/content are exported, never the absolute source path.
    pub source: PathBuf,
    /// Operator confirms input writers are stopped; this is not a live snapshot.
    #[arg(long, required = true)]
    pub quiesced_input: bool,
    #[arg(long)]
    pub snapshot: String,
    #[arg(long)]
    pub backup_type: String,
    #[arg(long, default_value = "datastore-default")]
    pub datastore: String,
    /// HTTPS or numeric loopback HTTP, PMBS_API_TOKEN, no redirects or proxies.
    #[arg(long, default_value = "http://127.0.0.1:8007")]
    pub remote: String,
    #[arg(long)]
    pub max_bytes: u64,
    #[arg(long)]
    pub max_files: usize,
    #[arg(long, default_value_t = 1024 * 1024 * 1024u64)]
    pub max_file_bytes: u64,
    /// Cooperative capture deadline; cannot interrupt a stalled kernel filesystem syscall.
    #[arg(long, default_value_t = 300)]
    pub max_seconds: u64,
}
