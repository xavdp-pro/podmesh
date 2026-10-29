//! PMBS Datastore Management
//! Manages deduplicated Content-Addressable Storage (CAS 4MB BLAKE3),
//! backup snapshot manifests, and disk metrics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB canonical chunks

static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Computes BLAKE3 lower hex digest (canonical CAS chunk & file digest for PMBS v2.4+).
pub fn blake3_digest(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// Verifies chunk/file integrity against BLAKE3.
pub fn verify_cas_digest(data: &[u8], expected_clean: &str) -> bool {
    blake3_digest(data) == expected_clean
}

pub fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidInput, message) }
pub fn validate_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.len() > 128 || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
        return Err(invalid("invalid path component"));
    }
    Ok(())
}
pub fn validate_file_path(path: &str) -> io::Result<()> {
    if path.is_empty() || path.len() > 4096 || path.chars().any(|c| c.is_control() || c == '\\' || c == ':')
        || path.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(invalid("invalid relative file path"));
    }
    Ok(())
}
pub fn validate_chunk_id(id: &str) -> io::Result<()> {
    let id = id.strip_prefix("blake3:").unwrap_or(id);
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(invalid("invalid BLAKE3 chunk id"));
    }
    Ok(())
}
fn secure_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid("non-directory or symlink in storage path")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let m = fs::symlink_metadata(path)?;
                    if m.is_dir() && !m.file_type().is_symlink() { Ok(()) }
                    else { Err(invalid("non-directory or symlink in storage path")) }
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}
fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
    if !f.metadata()?.is_file() { return Err(invalid("storage object is not a regular file")); }
    let mut bytes = Vec::new(); f.read_to_end(&mut bytes)?; Ok(bytes)
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
        validate_name(name)?;
        if fs::symlink_metadata(base_dir).map(|m| m.file_type().is_symlink()).unwrap_or(false) { return Err(invalid("symlink base directory")); }
        fs::create_dir_all(base_dir)?;
        secure_dir(&base_dir.join("datastores"))?;
        let root_dir = base_dir.join("datastores").join(name);
        let chunks_dir = root_dir.join("chunks");
        let snapshots_dir = root_dir.join("snapshots");

        secure_dir(&root_dir)?;
        secure_dir(&chunks_dir)?;
        secure_dir(&snapshots_dir)?;

        Ok(Self {
            name: name.to_string(),
            root_dir,
            chunks_dir,
            snapshots_dir,
        })
    }

    /// Normalizes chunk ID string by stripping any "blake3:" prefix.
    pub fn normalize_id(id: &str) -> &str {
        id.strip_prefix("blake3:").unwrap_or(id)
    }

    /// Computes path for chunk storage: `<chunks_dir>/<prefix>/<hash>`
    pub fn chunk_path(&self, chunk_id: &str) -> io::Result<PathBuf> {
        validate_chunk_id(chunk_id)?;
        let clean = Self::normalize_id(chunk_id);
        let prefix = if clean.len() >= 2 { &clean[..2] } else { "xx" };
        Ok(self.chunks_dir.join(prefix).join(clean))
    }

    /// Checks if a chunk exists locally.
    pub fn has_chunk(&self, chunk_id: &str) -> bool {
        self.chunk_path(chunk_id).map(|p| p.is_file()).unwrap_or(false)
    }

    /// Stores a 4MB chunk using BLAKE3 CAS addressing. If already present, skips write (deduplication).
    pub fn write_chunk(&self, data: &[u8]) -> io::Result<String> {
        if data.is_empty() || data.len() > CHUNK_SIZE { return Err(invalid("invalid chunk size")); }
        let digest = blake3_digest(data);
        let target_file = self.chunk_path(&digest)?;

        if target_file.exists() {
            self.read_chunk(&digest)?;
            return Ok(format!("blake3:{}", digest));
        }

        if let Some(parent) = target_file.parent() {
            secure_dir(parent)?;
        }

        let seq = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_file = target_file.with_extension(format!("tmp.{}.{}.{}", std::process::id(), seq, nanos));

        {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp_file)?;
            file.write_all(data)?;
            file.sync_all()?;
        }

        fs::rename(tmp_file, &target_file)?;
        Ok(format!("blake3:{}", digest))
    }

    /// Reads a chunk and verifies cryptographic hash (BLAKE3 primary, SHA-256 legacy anti-bit-rot).
    pub fn read_chunk(&self, chunk_id: &str) -> io::Result<Vec<u8>> {
        let target_file = self.chunk_path(chunk_id)?;
        if !target_file.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Chunk introuvable: {}", chunk_id),
            ));
        }

        secure_dir(target_file.parent().unwrap())?;
        let buffer = read_regular(&target_file)?;
        if buffer.is_empty() || buffer.len() > CHUNK_SIZE { return Err(invalid("invalid stored chunk size")); }

        let expected = Self::normalize_id(chunk_id);
        if !verify_cas_digest(&buffer, expected) {
            let actual = blake3_digest(&buffer);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Bit-rot détecté sur {}: attendu={}, réel={}", chunk_id, expected, actual),
            ));
        }

        Ok(buffer)
    }

    /// Validate structure before any paths are created, then audit full file content.
    pub fn validate_manifest(&self, m: &BackupManifest) -> io::Result<()> {
        validate_name(&m.id)?; validate_name(&m.backup_type)?;
        chrono::DateTime::parse_from_rfc3339(&m.backup_time).map_err(|_| invalid("invalid backup timestamp"))?;
        if m.verified || m.verified_at.is_some() { return Err(invalid("caller cannot assert verification")); }
        let mut total = 0u64; let mut count = 0usize;
        if m.files.len() > 10000 { return Err(invalid("too many files")); }
        for (key, f) in &m.files {
            validate_file_path(key)?;
            if key != &f.path || f.chunks_count != f.chunks.len() { return Err(invalid("file identity or chunk count mismatch")); }
            if !f.digest.starts_with("blake3:") { return Err(invalid("file digest must be BLAKE3")); }
            validate_chunk_id(&f.digest)?;
            let mut offset = 0u64;
            let mut b3_hash = blake3::Hasher::new();
            for (idx, c) in f.chunks.iter().enumerate() {
                validate_chunk_id(&c.id)?;
                if c.offset != offset || c.size == 0 || c.size > CHUNK_SIZE
                    || (idx + 1 < f.chunks.len() && c.size != CHUNK_SIZE) { return Err(invalid("invalid chunk layout")); }
                offset = offset.checked_add(c.size as u64).ok_or_else(|| invalid("file size overflow"))?;
                let bytes = self.read_chunk(&c.id).map_err(|_| invalid("missing or corrupt referenced chunk"))?;
                if bytes.len() != c.size { return Err(invalid("chunk size mismatch")); }
                b3_hash.update(&bytes);
            }
            if offset != f.size || format!("blake3:{}", b3_hash.finalize().to_hex()) != f.digest { return Err(invalid("file size or digest mismatch")); }
            total = total.checked_add(f.size).ok_or_else(|| invalid("total size overflow"))?;
            count = count.checked_add(f.chunks.len()).ok_or_else(|| invalid("chunk count overflow"))?;
        }
        if total != m.total_logical_bytes || count != m.chunks_count { return Err(invalid("snapshot aggregate mismatch")); }
        Ok(())
    }
    /// Publish immutable snapshots: exact retries succeed, conflicting contents never replace.
    pub fn save_snapshot(&self, manifest: &BackupManifest) -> io::Result<()> {
        self.publish_snapshot(manifest, false)
    }
    /// Conditional create: even an exact existing retry is refused, atomically.
    pub fn create_snapshot(&self, manifest: &BackupManifest) -> io::Result<()> {
        self.publish_snapshot(manifest, true)
    }
    fn publish_snapshot(&self, manifest: &BackupManifest, create_only: bool) -> io::Result<()> {
        self.validate_manifest(manifest)?;
        let type_dir = self.snapshots_dir.join(&manifest.backup_type); secure_dir(&type_dir)?;
        let snap_dir = type_dir.join(&manifest.id); secure_dir(&snap_dir)?;
        if create_only {
            File::open(&self.snapshots_dir)?.sync_all()?;
            File::open(&type_dir)?.sync_all()?;
        }
        let target = snap_dir.join("manifest.json");
        let value = serde_json::to_value(manifest).map_err(|_| invalid("invalid manifest"))?;
        if create_only && fs::symlink_metadata(&target).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "snapshot identity already exists"));
        }
        if target.exists() {
            let previous: serde_json::Value = serde_json::from_slice(&read_regular(&target)?).map_err(|_| invalid("invalid existing manifest"))?;
            return if previous == value { Ok(()) } else { Err(io::Error::new(io::ErrorKind::AlreadyExists, "snapshot identity conflict")) };
        }
        let seq = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = snap_dir.join(format!("manifest.tmp.{}.{}", std::process::id(), seq));
        let mut temp_owned = false;
        let result = (|| {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?;
            temp_owned = true;
            file.write_all(&serde_json::to_vec_pretty(&value).map_err(|_| invalid("invalid manifest"))?)?;
            file.sync_all()?;
            fs::hard_link(&temp, &target)?;
            File::open(&snap_dir)?.sync_all()
        })();
        if temp_owned { let _ = fs::remove_file(&temp); }
        result
    }

    /// Loads all snapshot manifests within this datastore.
    pub fn list_snapshots(&self) -> Vec<BackupManifest> {
        let mut results = Vec::new();
        if !self.snapshots_dir.exists() {
            return results;
        }

        if let Ok(type_entries) = fs::read_dir(&self.snapshots_dir) {
            for type_entry in type_entries.flatten() {
                if type_entry.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
                    if validate_name(&type_entry.file_name().to_string_lossy()).is_err() { continue; }
                    if let Ok(snap_entries) = fs::read_dir(type_entry.path()) {
                        for snap_entry in snap_entries.flatten() {
                            if !snap_entry.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) || validate_name(&snap_entry.file_name().to_string_lossy()).is_err() { continue; }
                            let manifest_path = snap_entry.path().join("manifest.json");
                            if manifest_path.exists() {
                                if let Ok(content) = read_regular(&manifest_path).and_then(|v| String::from_utf8(v).map_err(|_| invalid("invalid manifest encoding"))) {
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
        assert!(chunk_id.starts_with("blake3:"));

        // Deduplication test: re-writing the same data returns same ID without duplicate storage
        let chunk_id_dup = ds.write_chunk(data).unwrap();
        assert_eq!(chunk_id, chunk_id_dup);

        // Verification of read data
        let read_back = ds.read_chunk(&chunk_id).unwrap();
        assert_eq!(read_back, data);

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
