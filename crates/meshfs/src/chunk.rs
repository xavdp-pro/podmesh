//! CAS (Content-Addressable Storage) Chunk Store for MeshFS
//! Splits data into immutable 4MB chunks indexed by their SHA-256 digests.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB canonical chunks

/// Computes SHA-256 digest string in lower hex.
pub fn sha256_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Metadata describing a chunk slice within a file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    pub id: String,
    pub size: usize,
    pub offset: u64,
}

/// Disk-backed immutable Content-Addressable Storage for chunks.
#[derive(Debug, Clone)]
pub struct ChunkStore {
    pub chunks_dir: PathBuf,
}

impl ChunkStore {
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        let chunks_dir = data_dir.join("chunks");
        fs::create_dir_all(&chunks_dir)?;
        Ok(Self { chunks_dir })
    }

    /// Strips any "sha256:" prefix and ensures clean lowercase hash.
    pub fn normalize_id(id: &str) -> &str {
        id.strip_prefix("sha256:").unwrap_or(id)
    }

    /// Resolves disk path for a given chunk hash: <chunks_dir>/<prefix>/<hash>
    pub fn chunk_path(&self, chunk_id: &str) -> PathBuf {
        let clean = Self::normalize_id(chunk_id);
        let prefix = if clean.len() >= 2 { &clean[..2] } else { "xx" };
        self.chunks_dir.join(prefix).join(clean)
    }

    /// Checks if a chunk already exists locally.
    pub fn has_chunk(&self, chunk_id: &str) -> bool {
        self.chunk_path(chunk_id).exists()
    }

    /// Atomically writes a chunk to disk. If identical chunk already exists, reuses it without re-writing.
    pub fn write_chunk(&self, data: &[u8]) -> io::Result<String> {
        let digest = sha256_digest(data);
        let target_file = self.chunk_path(&digest);

        if target_file.exists() && self.read_chunk(&digest).is_ok() {
            return Ok(format!("sha256:{}", digest));
        }

        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }

        use std::sync::atomic::{AtomicU64, Ordering};
        static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_file =
            target_file.with_extension(format!("tmp.{}.{}.{}", std::process::id(), seq, nanos));
        {
            let mut file = File::create(&tmp_file)?;
            file.write_all(data)?;
            file.sync_all()?;
        }

        fs::rename(tmp_file, &target_file)?;
        File::open(target_file.parent().expect("chunk parent"))?.sync_all()?;
        File::open(&self.chunks_dir)?.sync_all()?;
        Ok(format!("sha256:{}", digest))
    }

    /// Reads chunk content from disk and verifies integrity.
    pub fn read_chunk(&self, chunk_id: &str) -> io::Result<Vec<u8>> {
        let id = Self::normalize_id(chunk_id);
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid chunk digest",
            ));
        }
        let target_file = self.chunk_path(chunk_id);
        if !target_file.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Chunk introuvable sur MeshFS: {}", chunk_id),
            ));
        }

        let file = File::open(&target_file)?;
        let mut buffer = Vec::new();
        file.take(CHUNK_SIZE as u64 + 1).read_to_end(&mut buffer)?;
        if buffer.len() > CHUNK_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized CAS chunk",
            ));
        }

        // Verify cryptographic hash on read (protection against bit-rot)
        let expected = Self::normalize_id(chunk_id);
        let actual = sha256_digest(&buffer);
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Corruption détectée (bit-rot) sur {}: attendu={}, réel={}",
                    chunk_id, expected, actual
                ),
            ));
        }

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_write_read_dedup() {
        let tmp_dir = std::env::temp_dir().join(format!("meshfs-test-{}", std::process::id()));
        let store = ChunkStore::new(&tmp_dir).unwrap();

        let data1 = b"Hello Pure Rust MeshFS 2026!";
        let chunk_id1 = store.write_chunk(data1).unwrap();
        assert!(chunk_id1.starts_with("sha256:"));

        // Verify deduplication: writing exact same data returns identical ID
        let chunk_id2 = store.write_chunk(data1).unwrap();
        assert_eq!(chunk_id1, chunk_id2);

        // Read and verify
        let read_back = store.read_chunk(&chunk_id1).unwrap();
        assert_eq!(read_back, data1);

        // Clean up
        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
