//! Merkle Tree DAG and Inode structures for MeshFS
//! Provides cryptographic verification of volume hierarchy and instant delta diffing.

use crate::chunk::{blake3_digest, ChunkRef};
use std::collections::{BTreeMap, HashMap};

/// Represents an immutable or mutable file Inode on MeshFS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Inode {
    #[serde(default)]
    pub revision: u64,
    pub path: String,
    pub size: u64,
    pub digest: String,
    #[serde(rename = "chunksCount")]
    pub chunks_count: usize,
    pub chunks: Vec<ChunkRef>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Result of a differential diff between two Merkle trees.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MerkleDiff {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub unchanged: Vec<String>,
    pub new_chunks_needed: Vec<String>,
}

/// Computes the canonical Merkle root hash for a set of file Inodes using BLAKE3.
pub fn compute_merkle_root(files: &HashMap<String, Inode>) -> String {
    if files.is_empty() {
        return format!("blake3:{}", blake3_digest(b""));
    }

    // Sort files deterministically in a BTreeMap
    let sorted: BTreeMap<&String, &Inode> = files.iter().collect();
    let mut lines = Vec::with_capacity(sorted.len());

    for (path, inode) in sorted {
        lines.push(format!("{}:{}:{}", path, inode.digest, inode.size));
    }

    let payload = lines.join("\n");
    let root = blake3_digest(payload.as_bytes());
    format!("blake3:{}", root)
}

/// Calculates diff between two file trees (e.g. for differential snapshot sync or backup).
pub fn diff_trees(base: &HashMap<String, Inode>, target: &HashMap<String, Inode>) -> MerkleDiff {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut unchanged = Vec::new();
    let mut new_chunks_needed = Vec::new();

    let mut existing_chunk_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for inode in base.values() {
        for c in &inode.chunks {
            existing_chunk_ids.insert(c.id.clone());
        }
    }

    for (path, target_inode) in target {
        match base.get(path) {
            None => {
                added.push(path.clone());
                for c in &target_inode.chunks {
                    if !existing_chunk_ids.contains(&c.id) {
                        new_chunks_needed.push(c.id.clone());
                    }
                }
            }
            Some(base_inode) => {
                if base_inode.digest != target_inode.digest || base_inode.size != target_inode.size
                {
                    modified.push(path.clone());
                    for c in &target_inode.chunks {
                        if !existing_chunk_ids.contains(&c.id) {
                            new_chunks_needed.push(c.id.clone());
                        }
                    }
                } else {
                    unchanged.push(path.clone());
                }
            }
        }
    }

    for path in base.keys() {
        if !target.contains_key(path) {
            deleted.push(path.clone());
        }
    }

    new_chunks_needed.sort();
    new_chunks_needed.dedup();

    MerkleDiff {
        added,
        modified,
        deleted,
        unchanged,
        new_chunks_needed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_root_determinism() {
        let mut files = HashMap::new();
        files.insert(
            "etc/hostname".to_string(),
            Inode {
                revision: 0,
                path: "etc/hostname".to_string(),
                size: 12,
                digest: "blake3:1111".to_string(),
                chunks_count: 1,
                chunks: vec![],
                updated_at: "2026-10-19T00:00:00Z".to_string(),
            },
        );
        files.insert(
            "var/log/syslog".to_string(),
            Inode {
                revision: 0,
                path: "var/log/syslog".to_string(),
                size: 2048,
                digest: "blake3:2222".to_string(),
                chunks_count: 1,
                chunks: vec![],
                updated_at: "2026-10-19T00:00:00Z".to_string(),
            },
        );

        let root1 = compute_merkle_root(&files);
        let root2 = compute_merkle_root(&files);
        assert_eq!(root1, root2);
        assert!(root1.starts_with("blake3:"));
    }

    #[test]
    fn test_diff_trees() {
        let mut base = HashMap::new();
        base.insert(
            "file1.txt".to_string(),
            Inode {
                revision: 0,
                path: "file1.txt".to_string(),
                size: 10,
                digest: "blake3:aaaa".to_string(),
                chunks_count: 1,
                chunks: vec![ChunkRef {
                    id: "blake3:aaaa".to_string(),
                    size: 10,
                    offset: 0,
                }],
                updated_at: "2026-10-19T00:00:00Z".to_string(),
            },
        );

        let mut target = base.clone();
        // Modify file1
        target.insert(
            "file1.txt".to_string(),
            Inode {
                revision: 0,
                path: "file1.txt".to_string(),
                size: 15,
                digest: "blake3:bbbb".to_string(),
                chunks_count: 1,
                chunks: vec![ChunkRef {
                    id: "blake3:bbbb".to_string(),
                    size: 15,
                    offset: 0,
                }],
                updated_at: "2026-10-19T01:00:00Z".to_string(),
            },
        );
        // Add file2
        target.insert(
            "file2.txt".to_string(),
            Inode {
                revision: 0,
                path: "file2.txt".to_string(),
                size: 20,
                digest: "blake3:cccc".to_string(),
                chunks_count: 1,
                chunks: vec![ChunkRef {
                    id: "blake3:cccc".to_string(),
                    size: 20,
                    offset: 0,
                }],
                updated_at: "2026-10-19T01:00:00Z".to_string(),
            },
        );

        let diff = diff_trees(&base, &target);
        assert_eq!(diff.modified, vec!["file1.txt"]);
        assert_eq!(diff.added, vec!["file2.txt"]);
        assert_eq!(diff.new_chunks_needed.len(), 2);
    }
}
