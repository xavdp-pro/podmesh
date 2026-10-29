//! CAS (Content-Addressable Storage) Chunk Store for MeshFS
//! Splits data into immutable 4MB chunks indexed by their BLAKE3 digests (with SHA-256 backward compatibility).

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB canonical chunks

/// Computes BLAKE3 digest string in lower hex (primary CAS chunk digest for MeshFS v2.4+).
pub fn blake3_digest(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Computes SHA-256 digest string in lower hex (legacy v2.3 compatibility & metadata).
pub fn sha256_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Verifies chunk integrity against BLAKE3 (primary v2.4+) or SHA-256 (backward-compatible v2.3).
pub fn verify_cas_digest(data: &[u8], expected_clean: &str) -> bool {
    blake3_digest(data) == expected_clean || sha256_digest(data) == expected_clean
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
    pub peers: Vec<String>,
    pub self_healed_chunks: Arc<AtomicU64>,
}

impl ChunkStore {
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        let mut peers: Vec<String> = std::env::var("MESHFS_PEERS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if peers.is_empty() {
            if let Ok(cfg_str) = fs::read_to_string("/etc/meshfs/config.json") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cfg_str) {
                    if let Some(arr) = val.get("peers").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                if !s.trim().is_empty() {
                                    peers.push(s.trim().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        Self::with_peers(data_dir, peers)
    }

    pub fn with_peers(data_dir: &Path, peers: Vec<String>) -> io::Result<Self> {
        let chunks_dir = data_dir.join("chunks");
        fs::create_dir_all(&chunks_dir)?;
        Ok(Self {
            chunks_dir,
            peers,
            self_healed_chunks: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Strips any "blake3:" or "sha256:" prefix and ensures clean lowercase hash.
    pub fn normalize_id(id: &str) -> &str {
        id.strip_prefix("blake3:")
            .or_else(|| id.strip_prefix("sha256:"))
            .unwrap_or(id)
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

    pub fn write_verified_chunk_raw(&self, clean_digest: &str, data: &[u8]) -> io::Result<()> {
        let target_file = self.chunk_path(clean_digest);
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)?;
        }

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
        Ok(())
    }

    /// Atomically writes a chunk to disk using BLAKE3 CAS addressing. If identical chunk already exists, reuses it without re-writing.
    pub fn write_chunk(&self, data: &[u8]) -> io::Result<String> {
        let digest = blake3_digest(data);
        let target_file = self.chunk_path(&digest);

        if target_file.exists() {
            if let Ok(existing) = fs::read(&target_file) {
                if existing.len() <= CHUNK_SIZE && blake3_digest(&existing) == digest {
                    return Ok(format!("blake3:{}", digest));
                }
            }
        }

        self.write_verified_chunk_raw(&digest, data)?;
        Ok(format!("blake3:{}", digest))
    }

    fn try_heal_from_peers(&self, clean: &str) -> Option<Vec<u8>> {
        if self.peers.is_empty() {
            return None;
        }
        let clean_owned = clean.to_string();
        let peers = self.peers.clone();

        let fetched = std::thread::spawn(move || -> Option<Vec<u8>> {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .ok()?;
            for peer in &peers {
                let base = peer.trim_end_matches('/');
                let candidate_urls = [
                    format!("{}/api/v1/meshfs/cas/chunk/{}", base, clean_owned),
                    format!("{}/cas/chunk/{}", base, clean_owned),
                ];
                for url in &candidate_urls {
                    if let Ok(resp) = client.get(url).send() {
                        if resp.status().is_success() {
                            if let Ok(bytes) = resp.bytes() {
                                if bytes.len() <= CHUNK_SIZE
                                    && verify_cas_digest(&bytes, &clean_owned)
                                {
                                    tracing::warn!(
                                        chunk = %clean_owned,
                                        peer = %base,
                                        "MeshFS P2P Self-Healing: recovered & verified intact CAS chunk from peer"
                                    );
                                    return Some(bytes.to_vec());
                                }
                            }
                        }
                    }
                }
            }
            None
        })
        .join()
        .ok()
        .flatten()?;

        if self.write_verified_chunk_raw(clean, &fetched).is_ok() {
            self.self_healed_chunks.fetch_add(1, Ordering::Relaxed);
            return Some(fetched);
        }
        None
    }

    /// Reads chunk content from disk and verifies integrity (with automatic P2P self-healing if peers are configured).
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
            if let Some(healed) = self.try_heal_from_peers(id) {
                return Ok(healed);
            }
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Chunk introuvable sur MeshFS: {}", chunk_id),
            ));
        }

        let file = File::open(&target_file)?;
        let mut buffer = Vec::new();
        file.take(CHUNK_SIZE as u64 + 1).read_to_end(&mut buffer)?;
        if buffer.len() > CHUNK_SIZE {
            if let Some(healed) = self.try_heal_from_peers(id) {
                return Ok(healed);
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized CAS chunk",
            ));
        }

        // Verify cryptographic hash on read (BLAKE3 primary, SHA-256 legacy compatibility)
        let expected = Self::normalize_id(chunk_id);
        if !verify_cas_digest(&buffer, expected) {
            let actual = blake3_digest(&buffer);
            if let Some(healed) = self.try_heal_from_peers(id) {
                return Ok(healed);
            }
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

    pub fn count_chunks_and_bytes(&self) -> (usize, u64) {
        let mut count = 0usize;
        let mut total_bytes = 0u64;
        if let Ok(entries) = fs::read_dir(&self.chunks_dir) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    if let Ok(sub) = fs::read_dir(e.path()) {
                        for f in sub.flatten() {
                            let p = f.path();
                            if p.is_file()
                                && !f.file_name().to_string_lossy().contains(".tmp.")
                            {
                                count += 1;
                                if let Ok(meta) = f.metadata() {
                                    total_bytes += meta.len();
                                }
                            }
                        }
                    }
                }
            }
        }
        (count, total_bytes)
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
        assert!(chunk_id1.starts_with("blake3:"));

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
