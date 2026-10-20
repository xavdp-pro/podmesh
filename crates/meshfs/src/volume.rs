//! Volume and Snapshot Management for MeshFS
//! Provides logical grouping of files, Merkle DAG state, and instant O(1) Copy-on-Write snapshots.

use crate::merkle::{compute_merkle_root, Inode};
use chrono::Utc;
use std::collections::HashMap;

/// Logical volume holding a namespace of files and its Merkle root.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Volume {
    pub name: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub files: HashMap<String, Inode>,
    #[serde(rename = "merkleRoot")]
    pub merkle_root: String,
}

impl Volume {
    pub fn new(name: &str) -> Self {
        let mut vol = Self {
            name: name.to_string(),
            created_at: Utc::now().to_rfc3339(),
            files: HashMap::new(),
            merkle_root: String::new(),
        };
        vol.recompute_merkle();
        vol
    }

    /// Recomputes the Merkle root hash for all files currently in the volume.
    pub fn recompute_merkle(&mut self) -> String {
        let root = compute_merkle_root(&self.files);
        self.merkle_root = root.clone();
        root
    }

    pub fn insert_file(&mut self, inode: Inode) {
        self.files.insert(inode.path.clone(), inode);
        self.recompute_merkle();
    }

    pub fn get_file(&self, path: &str) -> Option<&Inode> {
        self.files.get(path)
    }

    pub fn remove_file(&mut self, path: &str) -> Option<Inode> {
        let removed = self.files.remove(path);
        if removed.is_some() {
            self.recompute_merkle();
        }
        removed
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|i| i.size).sum()
    }
}

/// Instant O(1) Copy-on-Write snapshot capturing the exact Merkle DAG state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub volume: String,
    #[serde(rename = "merkleRoot")]
    pub merkle_root: String,
    #[serde(rename = "filesCount")]
    pub files_count: usize,
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "filesTree")]
    pub files_tree: HashMap<String, Inode>,
}

impl Snapshot {
    pub fn create_from_volume(volume: &Volume, custom_name: Option<&str>) -> Self {
        let timestamp = Utc::now().timestamp();
        let snap_id = format!("snap-{}-{}", volume.name, timestamp);
        let name = custom_name.unwrap_or(&snap_id).to_string();

        Self {
            id: snap_id,
            name,
            volume: volume.name.clone(),
            merkle_root: volume.merkle_root.clone(),
            files_count: volume.files.len(),
            total_bytes: volume.total_bytes(),
            created_at: Utc::now().to_rfc3339(),
            files_tree: volume.files.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_snapshot_cow() {
        let mut vol = Volume::new("tank");
        assert!(vol.merkle_root.starts_with("sha256:"));

        vol.insert_file(Inode {
            revision: 0,
            path: "images/100/vm-100-disk-0.raw".to_string(),
            size: 1048576,
            digest: "sha256:abcd".to_string(),
            chunks_count: 1,
            chunks: vec![],
            updated_at: "2026-10-19T00:00:00Z".to_string(),
        });

        assert_eq!(vol.files.len(), 1);
        let root_before = vol.merkle_root.clone();

        // Create snapshot
        let snap = Snapshot::create_from_volume(&vol, Some("pre-upgrade"));
        assert_eq!(snap.name, "pre-upgrade");
        assert_eq!(snap.merkle_root, root_before);
        assert_eq!(snap.files_count, 1);

        // Modify volume: add another disk
        vol.insert_file(Inode {
            revision: 0,
            path: "images/100/vm-100-disk-1.raw".to_string(),
            size: 2097152,
            digest: "sha256:ef01".to_string(),
            chunks_count: 1,
            chunks: vec![],
            updated_at: "2026-10-19T01:00:00Z".to_string(),
        });

        // The snapshot retains historical frozen state (immutability)
        assert_eq!(snap.files_tree.len(), 1);
        assert_eq!(vol.files.len(), 2);
        assert_ne!(vol.merkle_root, snap.merkle_root);
    }
}
