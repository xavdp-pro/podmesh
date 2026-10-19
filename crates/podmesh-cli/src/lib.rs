//! PodMesh CLI & Orchestration Engine Root (Pure Rust)

pub mod admin;
pub mod crypto;
pub mod ha;
pub mod move_cmd;
pub mod replication;
pub mod system;

pub use admin::{get_cluster_status, ClusterStatusReport};
pub use crypto::{canonicalize_json, verify_canonical_json, AuthorityKey};
pub use ha::{AuthorityPolicy, HALedger, TakeoverProof};
pub use move_cmd::{execute_move, MoveReport};
pub use replication::{ReplicationConfig, ReplicationManager, ReplicationReport};
pub use system::{read_mandate, run_boot_restore, run_self_fence, FenceOutcome, RestoreOutcome};
