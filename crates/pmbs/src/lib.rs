//! PMBS — PodMesh Backup Server Library Root
//!
//! Provides:
//! - Content-Addressable Storage (CAS 4MB SHA-256) Datastores
//! - Op 10 Sandbox Boot Proof verification engine
//! - Inter-site differential replication
//! - Axum REST API on port 8007
//! - Unix Domain Socket IPC control

pub mod cli;
pub mod control;
pub mod datastore;
pub mod server;
pub mod sync;
pub mod verify;

pub use datastore::{BackupFile, BackupManifest, ChunkRef, Datastore, CHUNK_SIZE};
pub use server::{create_router, AppState, StatusResponse};
pub use sync::{sync_from_remote, SyncReport};
pub use verify::{verify_snapshot, VerificationReport};
