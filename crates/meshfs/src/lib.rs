//! MeshFS — Distributed CAS Filesystem and Engine for PodMesh and Proxmox VE
//!
//! Provides:
//! - Content-Addressable Storage (CAS) with 4MB SHA-256 chunking
//! - Multi-backend storage driver (ZFS, Btrfs, LVM Thin, POSIX) with kernel `ioctl(FICLONE)` zero-copy reflink
//! - Merkle Tree deterministic diffing and validation
//! - Instant Copy-on-Write snapshots capturing DAG states
//! - REST API compatible with Proxmox VE Storage Plugin (`MeshFSPlugin.pm`)

pub mod chunk;
pub mod cli;
pub mod cluster;
pub mod control;
pub mod engine;
pub mod merkle;
pub mod server;
pub mod storage;
pub mod volume;

pub use chunk::{blake3_digest, verify_cas_digest, ChunkRef, ChunkStore, CHUNK_SIZE};
pub use control::{query_control_socket, start_control_socket, ControlRequest, IntegrityReport};
pub use engine::{MeshFSEngine, NodeInfo, StatusReport};
pub use merkle::{compute_merkle_root, diff_trees, Inode, MerkleDiff};
pub use storage::{BackendFeatures, BackendInfo, StorageDriver, StorageMetrics};
pub use volume::{Snapshot, Volume};

pub mod live;
pub mod fuse;
pub mod docker_plugin;
