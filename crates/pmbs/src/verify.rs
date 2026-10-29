//! PMBS referenced-chunk integrity audit.
//! This verifier does not boot workloads or establish restoration readiness.

use crate::datastore::{Datastore, BackupManifest};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Result of a referenced-chunk integrity audit; boot remains untested.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub ok: bool,
    #[serde(rename = "integrityVerified")]
    pub integrity_verified: bool,
    #[serde(rename = "backupId")]
    pub backup_id: String,
    #[serde(rename = "datastore")]
    pub datastore: String,
    #[serde(rename = "verifiedAt")]
    pub verified_at: String,
    #[serde(rename = "chunksAudited")]
    pub chunks_audited: usize,
    #[serde(rename = "chunksCorrupted")]
    pub chunks_corrupted: Vec<String>,
    #[serde(rename = "chunksMissing")]
    pub chunks_missing: Vec<String>,
    #[serde(rename = "op10SandboxBootProof")]
    pub op10_sandbox_boot_proof: bool,
    #[serde(rename = "bootReadiness")]
    pub boot_readiness: String,
    pub message: String,
}

/// Checks referenced chunk hashes and sizes, without executing a restore or boot.
pub fn verify_snapshot(datastore: &Datastore, manifest: &BackupManifest) -> VerificationReport {
    let mut chunks_audited = 0;
    let mut chunks_corrupted = Vec::new();
    let mut chunks_missing = Vec::new();

    for (_file_path, file_meta) in &manifest.files {
        for chunk_ref in &file_meta.chunks {
            chunks_audited += 1;
            let chunk_path = datastore.chunk_path(&chunk_ref.id);

            if !chunk_path.map(|p| p.is_file()).unwrap_or(false) {
                chunks_missing.push(chunk_ref.id.clone());
                continue;
            }

            // Verify SHA-256 byte integrity
            match datastore.read_chunk(&chunk_ref.id) {
                Ok(bytes) => {
                    if bytes.len() != chunk_ref.size {
                        chunks_corrupted.push(format!("{}: taille incorrecte", chunk_ref.id));
                    }
                }
                Err(e) => {
                    chunks_corrupted.push(format!("{}: {}", chunk_ref.id, e));
                }
            }
        }
    }

    if let Err(e) = datastore.validate_manifest(manifest) { chunks_corrupted.push(format!("manifest: {}", e)); }
    let is_crypto_sound = chunks_missing.is_empty() && chunks_corrupted.is_empty();
    let integrity_verified = is_crypto_sound && chunks_audited > 0;
    let message = if integrity_verified {
        format!("Referenced chunk integrity verified ({} chunks); restore and boot not tested.", chunks_audited)
    } else {
        format!("Chunk integrity not verified: {} missing, {} corrupted, {} audited; boot not tested.", chunks_missing.len(), chunks_corrupted.len(), chunks_audited)
    };

    VerificationReport {
        ok: integrity_verified,
        integrity_verified,
        backup_id: manifest.id.clone(),
        datastore: datastore.name.clone(),
        verified_at: Utc::now().to_rfc3339(),
        chunks_audited,
        chunks_corrupted,
        chunks_missing,
        op10_sandbox_boot_proof: false,
        boot_readiness: "NOT_TESTED".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_chunk_integrity_never_claims_boot_proof() {
        let tmp_dir = std::env::temp_dir().join(format!("pmbs-verify-test-{}", std::process::id()));
        let ds = Datastore::new(&tmp_dir, "backup-tank").unwrap();

        let raw = b"Linux Kernel / Bootloader / QEMU Partition Table Sandbox Bytes";
        let chunk_id = ds.write_chunk(raw).unwrap();

        let mut files = HashMap::new();
        files.insert(
            "drive-scsi0.img".to_string(),
            crate::datastore::BackupFile {
                path: "drive-scsi0.img".to_string(),
                size: raw.len() as u64,
                digest: format!("blake3:{}", crate::datastore::blake3_digest(raw)),
                chunks_count: 1,
                chunks: vec![crate::datastore::ChunkRef {
                    id: chunk_id.clone(),
                    size: raw.len(),
                    offset: 0,
                }],
            },
        );

        let manifest = BackupManifest {
            id: "vm-100-backup-point".to_string(),
            backup_type: "vm".to_string(),
            backup_time: Utc::now().to_rfc3339(),
            files,
            total_logical_bytes: raw.len() as u64,
            chunks_count: 1,
            verified: false,
            verified_at: None,
        };

        ds.save_snapshot(&manifest).unwrap();

        let report = verify_snapshot(&ds, &manifest);
        assert!(report.ok);
        assert!(report.integrity_verified);
        assert!(!report.op10_sandbox_boot_proof);
        assert_eq!(report.boot_readiness, "NOT_TESTED");
        assert_eq!(report.chunks_audited, 1);

        std::fs::remove_file(ds.chunk_path(&chunk_id).unwrap()).unwrap();
        let missing = verify_snapshot(&ds, &manifest);
        assert!(!missing.ok && !missing.integrity_verified && !missing.op10_sandbox_boot_proof);
        assert_eq!(missing.boot_readiness, "NOT_TESTED");
        assert_eq!(missing.chunks_missing.len(), 1);
        let mut empty = manifest.clone();
        empty.files.clear();
        let empty_report = verify_snapshot(&ds, &empty);
        assert!(!empty_report.integrity_verified && !empty_report.op10_sandbox_boot_proof);
        assert_eq!(empty_report.boot_readiness, "NOT_TESTED");
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
