//! PMBS Datastore Management
//! Manages deduplicated Content-Addressable Storage (CAS 4MB SHA-256),
//! backup snapshot manifests, and disk metrics.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB canonical chunks

static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Computes SHA-256 lower hex digest.
pub fn sha256_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Metadata slice reference to a deduplicated 4MB CAS chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    pub id: String,
    pub size: usize,
    pub offset: u64,
}

/// A backed-up file or disk image within a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupFile {
    pub path: String,
    pub size: u64,
    pub digest: String,
    #[serde(rename = "chunksCount")]
    pub chunks_count: usize,
    pub chunks: Vec<ChunkRef>,
}

/// Snapshot manifest recording backed-up entities at a specific point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub id: String,
    #[serde(rename = "backupType")]
    pub backup_type: String,
    #[serde(rename = "backupTime")]
    pub backup_time: String,
    pub files: HashMap<String, BackupFile>,
    #[serde(rename = "totalLogicalBytes")]
    pub total_logical_bytes: u64,
    #[serde(rename = "chunksCount")]
    pub chunks_count: usize,
    pub verified: bool,
    #[serde(rename = "verifiedAt")]
    pub verified_at: Option<String>,
}

/// High-performance deduplicated Datastore instance.
#[derive(Debug, Clone)]
pub struct Datastore {
    pub name: String,
    pub root_dir: PathBuf,
    pub chunks_dir: PathBuf,
    pub snapshots_dir: PathBuf,
}

impl Datastore {
    /// Opens or initializes a Datastore in `<base_dir>/datastores/<name>`.
    pub fn new(base_dir: &Path, name: &str) -> io::Result<Self> {
        let root_dir = base_dir.join("datastores").join(name);
        let chunks_dir = root_dir.join("chunks");
        let snapshots_dir = root_dir.join("snapshots");

        fs::create_dir_all(&chunks_dir)?;
        fs::create_dir_all(&snapshots_dir)?;

        Ok(Self {
            name: name.to_string(),
            root_dir,
            chunks_dir,
            snapshots_dir,
        })
    }

    /// Normalizes chunk ID string by stripping any "sha256:" prefix.
    pub fn normalize_id(id: &str) -> &str {
        id.strip_prefix("sha256:").unwrap_or(id)
    }

    /// Computes path for chunk storage: `<chunks_dir>/<prefix>/<hash>`
    pub fn chunk_path(&self, chunk_id: &str) -> PathBuf {
        let clean = Self::normalize_id(chunk_id);
        let prefix = if clean.len() >= 2 { &clean[..2] } else { "xx" };
        self.chunks_dir.join(prefix).join(clean)
    }

    /// Checks if a chunk exists locally.
    pub fn has_chunk(&self, chunk_id: &str) -> bool {
        self.chunk_path(chunk_id).exists()
    }

    /// Stores a 4MB chunk. If already present, skips write (deduplication).
    pub fn write_chunk(&self, data: &[u8]) -> io::Result<String> {
        let digest = sha256_digest(data);
        let target_file = self.chunk_path(&digest);

        if target_file.exists() {
            return Ok(format!("sha256:{}", digest));
        }

        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let seq = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_file = target_file.with_extension(format!("tmp.{}.{}.{}", std::process::id(), seq, nanos));

        {
            let mut file = File::create(&tmp_file)?;
            file.write_all(data)?;
            file.sync_all()?;
        }

        fs::rename(tmp_file, &target_file)?;
        Ok(format!("sha256:{}", digest))
    }

    /// Reads a chunk and verifies cryptographic hash (anti-bit-rot).
    pub fn read_chunk(&self, chunk_id: &str) -> io::Result<Vec<u8>> {
        let target_file = self.chunk_path(chunk_id);
        if !target_file.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Chunk introuvable: {}", chunk_id),
            ));
        }

        let mut file = File::open(&target_file)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let expected = Self::normalize_id(chunk_id);
        let actual = sha256_digest(&buffer);
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Bit-rot détecté sur {}: attendu={}, réel={}", chunk_id, expected, actual),
            ));
        }

        Ok(buffer)
    }

    /// Records a new backup snapshot and stores its manifest atomically.
    pub fn save_snapshot(&self, manifest: &BackupManifest) -> io::Result<()> {
        let snap_dir = self.snapshots_dir.join(&manifest.backup_type).join(&manifest.id);
        fs::create_dir_all(&snap_dir)?;

        let manifest_file = snap_dir.join("manifest.json");
        let serialized = serde_json::to_string_pretty(manifest)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let seq = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_file = snap_dir.join(format!("manifest.json.tmp.{}.{}", std::process::id(), seq));
        {
            let mut file = File::create(&tmp_file)?;
            file.write_all(serialized.as_bytes())?;
            file.sync_all()?;
        }

        fs::rename(tmp_file, manifest_file)?;
        Ok(())
    }

    /// Loads all snapshot manifests within this datastore.
    pub fn list_snapshots(&self) -> Vec<BackupManifest> {
        let mut results = Vec::new();
        if !self.snapshots_dir.exists() {
            return results;
        }

        if let Ok(type_entries) = fs::read_dir(&self.snapshots_dir) {
            for type_entry in type_entries.flatten() {
                if type_entry.path().is_dir() {
                    if let Ok(snap_entries) = fs::read_dir(type_entry.path()) {
                        for snap_entry in snap_entries.flatten() {
                            let manifest_path = snap_entry.path().join("manifest.json");
                            if manifest_path.exists() {
                                if let Ok(content) = fs::read_to_string(&manifest_path) {
                                    if let Ok(manifest) = serde_json::from_str::<BackupManifest>(&content) {
                                        results.push(manifest);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        results.sort_by(|a, b| b.backup_time.cmp(&a.backup_time));
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datastore_chunking_and_deduplication() {
        let tmp_dir = std::env::temp_dir().join(format!("pmbs-test-{}", std::process::id()));
        let ds = Datastore::new(&tmp_dir, "test-store").unwrap();

        let data = b"PMBS Pure Rust Enterprise Backup 2026";
        let chunk_id = ds.write_chunk(data).unwrap();
        assert!(chunk_id.starts_with("sha256:"));

        // Deduplication test: re-writing the same data returns same ID without duplicate storage
        let chunk_id_dup = ds.write_chunk(data).unwrap();
        assert_eq!(chunk_id, chunk_id_dup);

        // Verification of read data
        let read_back = ds.read_chunk(&chunk_id).unwrap();
        assert_eq!(read_back, data);

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
