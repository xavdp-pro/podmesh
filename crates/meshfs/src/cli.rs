//! MeshFS Command Line Interface Definitions
//! Powered by Clap 4.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 7443;
pub const DEFAULT_DATA_DIR: &str = "/var/lib/meshfs";
pub const DEFAULT_CLUSTER: &str = "meshfs-cluster";
pub const DEFAULT_SOCKET: &str = "/var/lib/meshfs/meshfs.sock";

#[derive(Parser, Debug)]
#[command(
    name = "meshfs",
    author = "Xavier DP <xavier@xavdp.pro>",
    version = "2.4.1",
    about = "MeshFS — Ultra-lightweight Sovereign Distributed Filesystem for PodMesh & Proxmox VE",
    long_about = "MeshFS operates autonomously from 1 node to N nodes with zero Ceph dependencies.\nSplits files into immutable 4MB CAS SHA-256 chunks, manages Merkle trees and provides instant CoW snapshots."
)]
pub struct Cli {
    /// Enable verbose debug logging (or set RUST_LOG=debug)
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the MeshFS storage daemon and REST API
    Daemon(DaemonArgs),

    /// Display local node or cluster status
    Status(StatusArgs),

    /// Join an existing MeshFS cluster node
    Join(JoinArgs),

    /// Quick ping health check via Unix Socket or HTTP
    Ping(PingArgs),

    /// Deep cryptographic integrity audit & health check (detect bit-rot)
    Inspect(InspectArgs),

    /// Volume management commands
    Volume(VolumeCommand),

    /// Create or manage instant Merkle CoW snapshots
    Snapshot(SnapshotArgs),

    /// Store a file into MeshFS (4MB CAS chunked)
    Put(PutArgs),

    /// Retrieve a file from MeshFS
    Get(GetArgs),
    /// Mount an experimental flat-file volume (one writer per file).
    Mount(MountArgs),
    /// Universal CAS Mark-and-Sweep Garbage Collector across MeshFS, MeshFS-S3, and PMBS
    Gc(GcArgs),
    /// Manage logical volume quotas
    Quota(QuotaCommand),
}

#[derive(Args, Debug)]
pub struct GcArgs {
    /// Grace window in seconds (chunks modified more recently than this are never deleted)
    #[arg(long, default_value_t = 3600)]
    pub grace_seconds: u64,
    /// Only report unreferenced chunks without deleting them
    #[arg(long)]
    pub dry_run: bool,
    /// Unix Domain Socket path to query directly
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
    /// MeshFS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,
}

#[derive(Args, Debug)]
pub struct QuotaCommand {
    #[command(subcommand)]
    pub action: QuotaSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum QuotaSubcommand {
    /// Set maximum logical bytes allowed for a volume (0 = unlimited)
    Set {
        volume: String,
        #[arg(long)]
        max_bytes: u64,
        #[arg(short, long)]
        socket: Option<PathBuf>,
    },
    /// Get current logical usage and quota for a volume
    Get {
        volume: String,
        #[arg(short, long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Args, Debug)]
pub struct DaemonArgs {
    /// Explicit fixed membership JSON; omission keeps standalone operation.
    #[arg(long)]
    pub cluster_config: Option<PathBuf>,
    /// Initialize a new cluster on this node only. Never use for disk replacement.
    #[arg(long, requires = "cluster_config")]
    pub bootstrap: bool,
    /// Port to listen on for HTTP REST API
    #[arg(short, long, default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// Root directory for MeshFS metadata and CAS chunks
    #[arg(short, long, default_value = DEFAULT_DATA_DIR)]
    pub data_dir: PathBuf,

    /// Cluster identifier
    #[arg(short, long, default_value = DEFAULT_CLUSTER)]
    pub cluster: String,

    /// Path to Unix Domain Socket for low-latency local IPC control
    #[arg(short, long, default_value = DEFAULT_SOCKET)]
    pub socket: PathBuf,

    /// Path to Podman/Docker Volume Plugin Unix Domain Socket
    #[arg(long, default_value = "/run/docker/plugins/meshfs.sock")]
    pub plugin_socket: PathBuf,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// MeshFS HTTP daemon address to query
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,

    /// Unix Domain Socket path to query directly (bypasses HTTP)
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PingArgs {
    /// MeshFS HTTP daemon address to query
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,

    /// Unix Domain Socket path to query directly (bypasses HTTP)
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    /// Optional volume name to inspect (all volumes if omitted)
    #[arg(short, long)]
    pub volume: Option<String>,

    /// Perform full SHA-256 byte re-hash of every CAS chunk on disk
    #[arg(short = 's', long, default_value_t = true)]
    pub deep: bool,

    /// MeshFS HTTP daemon address to query
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,

    /// Unix Domain Socket path to query directly (bypasses HTTP)
    #[arg(short, long)]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct JoinArgs {
    /// Target node address (e.g. 192.168.1.10:7443)
    pub target: String,

    /// Security token (optional)
    #[arg(short, long, default_value = "")]
    pub token: String,

    /// Local MeshFS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,
}

#[derive(Args, Debug)]
pub struct VolumeCommand {
    #[command(subcommand)]
    pub action: VolumeSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum VolumeSubcommand {
    /// List all volumes
    List {
        #[arg(short, long, default_value = "http://127.0.0.1:7443")]
        remote: String,
    },
    /// Create a new volume
    Create {
        /// Name of the volume
        name: String,
        #[arg(short, long, default_value = "http://127.0.0.1:7443")]
        remote: String,
    },
}

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    /// Volume name to snapshot
    #[arg(short, long, default_value = "default")]
    pub volume: String,

    /// Custom name for the snapshot (defaults to auto-generated ID)
    #[arg(short, long)]
    pub name: Option<String>,

    /// MeshFS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,
}

#[derive(Args, Debug)]
pub struct PutArgs {
    /// Volume name
    #[arg(short, long, default_value = "default")]
    pub volume: String,

    /// Destination path within the volume
    pub path: String,

    /// Local file to upload
    pub file: PathBuf,

    /// MeshFS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,
}

#[derive(Args, Debug)]
pub struct GetArgs {
    /// Volume name
    #[arg(short, long, default_value = "default")]
    pub volume: String,

    /// File path within the volume
    pub path: String,

    /// Local destination file path (prints to stdout if omitted)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// MeshFS HTTP daemon address
    #[arg(short, long, default_value = "http://127.0.0.1:7443")]
    pub remote: String,
}

#[derive(Args, Debug)]
pub struct MountArgs {
    #[arg(long, required = true)]
    pub peer: Vec<String>,
    #[arg(long)]
    pub volume: String,
    pub mountpoint: PathBuf,
}
