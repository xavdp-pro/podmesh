//! MeshFS Control Socket & Diagnostic Engine
//! Provides local Unix Domain Socket IPC for low-latency out-of-band monitoring,
//! health checks, and deep cryptographic integrity audits.

use crate::chunk::sha256_digest;
use crate::engine::MeshFSEngine;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{debug, error, info, warn};

/// Control request sent over Unix Domain Socket or HTTP diagnostic endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum ControlRequest {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "inspect")]
    Inspect {
        #[serde(default)]
        volume: Option<String>,
        #[serde(default)]
        deep_sha_verify: bool,
    },
}

/// Result of a deep storage integrity inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityReport {
    pub ok: bool,
    pub verified_at: String,
    pub volumes_checked: usize,
    pub files_checked: usize,
    pub chunks_checked: usize,
    pub chunks_missing: Vec<String>,
    pub chunks_corrupted: Vec<String>,
    pub merkle_valid: bool,
    pub backend_type: String,
    pub reflink_supported: bool,
    pub message: String,
}

impl MeshFSEngine {
    /// Audits all chunks and Merkle trees across volumes to verify zero bit-rot.
    pub async fn audit_integrity(&self, volume_filter: Option<&str>, deep_sha_verify: bool) -> IntegrityReport {
        let state = self.state.read().await;
        let mut files_checked = 0;
        let mut chunks_checked = 0;
        let mut chunks_missing = Vec::new();
        let mut chunks_corrupted = Vec::new();
        let mut merkle_valid = true;
        let mut volumes_checked = 0;

        for (vol_name, vol) in &state.volumes {
            if let Some(target) = volume_filter {
                if vol_name != target {
                    continue;
                }
            }
            volumes_checked += 1;

            // 1. Verify volume Merkle root integrity
            let recalculated = crate::merkle::compute_merkle_root(&vol.files);
            if recalculated != vol.merkle_root {
                warn!(
                    volume = %vol_name,
                    expected = %vol.merkle_root,
                    actual = %recalculated,
                    "Incohérence Merkle DAG détectée sur le volume"
                );
                merkle_valid = false;
            }

            // 2. Verify all files and their CAS chunk slices
            for (path, inode) in &vol.files {
                files_checked += 1;

                for chunk_ref in &inode.chunks {
                    chunks_checked += 1;
                    let chunk_path = self.chunk_store.chunk_path(&chunk_ref.id);

                    if !chunk_path.exists() {
                        error!(path = %path, chunk = %chunk_ref.id, "Chunk manquant sur le disque");
                        chunks_missing.push(chunk_ref.id.clone());
                        continue;
                    }

                    if deep_sha_verify {
                        match fs::read(&chunk_path) {
                            Ok(bytes) => {
                                let clean_id = crate::chunk::ChunkStore::normalize_id(&chunk_ref.id);
                                let calculated = sha256_digest(&bytes);
                                if calculated != clean_id {
                                    error!(
                                        path = %path,
                                        chunk = %chunk_ref.id,
                                        expected = %clean_id,
                                        actual = %calculated,
                                        "Bit-rot détecté sur le chunk"
                                    );
                                    chunks_corrupted.push(chunk_ref.id.clone());
                                }
                            }
                            Err(e) => {
                                error!(path = %path, chunk = %chunk_ref.id, err = %e, "Erreur lecture chunk");
                                chunks_corrupted.push(chunk_ref.id.clone());
                            }
                        }
                    }
                }
            }
        }

        let is_ok = merkle_valid && chunks_missing.is_empty() && chunks_corrupted.is_empty();
        let message = if is_ok {
            format!(
                "Intégrité vérifiée à 100%: {} volumes, {} fichiers, {} chunks CAS conformes.",
                volumes_checked, files_checked, chunks_checked
            )
        } else {
            format!(
                "Anomalies détectées: {} chunks manquants, {} chunks corrompus, Merkle valide: {}",
                chunks_missing.len(),
                chunks_corrupted.len(),
                merkle_valid
            )
        };

        IntegrityReport {
            ok: is_ok,
            verified_at: chrono::Utc::now().to_rfc3339(),
            volumes_checked,
            files_checked,
            chunks_checked,
            chunks_missing,
            chunks_corrupted,
            merkle_valid,
            backend_type: self.driver.info.backend_type.clone(),
            reflink_supported: self.driver.info.features.reflink_support,
            message,
        }
    }
}

/// Spawns the background Unix Domain Socket listener.
pub async fn start_control_socket(engine: MeshFSEngine, socket_path: PathBuf) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Remove stale socket if present
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;
    info!(path = %socket_path.display(), "MeshFS Control Unix Socket actif");

    let engine = Arc::new(engine);

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let engine = Arc::clone(&engine);
                    tokio::spawn(async move {
                        let (reader, mut writer) = stream.split();
                        let mut buf_reader = BufReader::new(reader);
                        const MAX_CONTROL_LINE: usize = 65536;
                        let mut line = String::new();
                        while let Ok(n) = buf_reader.read_line(&mut line).await {
                            if n == 0 {
                                break;
                            }
                            if line.len() > MAX_CONTROL_LINE {
                                let err_reply = "{\"ok\":false,\"error\":\"Requête trop volumineuse (dépasse 64KB)\"}\n";
                                let _ = writer.write_all(err_reply.as_bytes()).await;
                                let _ = writer.flush().await;
                                break;
                            }
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                line.clear();
                                continue;
                            }

                            debug!(raw = %trimmed, "Commande reçue sur le socket de contrôle");

                            let response = match serde_json::from_str::<ControlRequest>(trimmed) {
                                Ok(ControlRequest::Ping) => serde_json::json!({
                                    "status": "pong",
                                    "service": "meshfs-pure-rust",
                                    "timestamp": chrono::Utc::now().to_rfc3339()
                                }),
                                Ok(ControlRequest::Status) => {
                                    let status = engine.get_status().await;
                                    serde_json::to_value(status).unwrap_or_default()
                                }
                                Ok(ControlRequest::Inspect {
                                    volume,
                                    deep_sha_verify,
                                }) => {
                                    let rep = engine.audit_integrity(volume.as_deref(), deep_sha_verify).await;
                                    serde_json::to_value(rep).unwrap_or_default()
                                }
                                Err(_) => {
                                    // Fallback text commands: "ping", "status", "inspect"
                                    match trimmed.to_lowercase().as_str() {
                                        "ping" => serde_json::json!({ "status": "pong" }),
                                        "status" => {
                                            let status = engine.get_status().await;
                                            serde_json::to_value(status).unwrap_or_default()
                                        }
                                        "inspect" => {
                                            let rep = engine.audit_integrity(None, true).await;
                                            serde_json::to_value(rep).unwrap_or_default()
                                        }
                                        _ => serde_json::json!({
                                            "ok": false,
                                            "error": format!("Commande inconnue: '{}'. Actions: ping, status, inspect", trimmed)
                                        }),
                                    }
                                }
                            };

                            let mut reply = serde_json::to_string(&response).unwrap_or_default();
                            reply.push('\n');
                            if let Err(e) = writer.write_all(reply.as_bytes()).await {
                                error!(err = %e, "Erreur écriture socket de contrôle");
                                break;
                            }
                            let _ = writer.flush().await;
                            line.clear();
                        }
                    });
                }
                Err(e) => {
                    warn!(err = %e, "Erreur acceptation socket de contrôle Unix");
                }
            }
        }
    });

    Ok(())
}

/// Helper client to query a local MeshFS Unix Domain Socket.
pub async fn query_control_socket(socket_path: &Path, req: &ControlRequest) -> Result<String, String> {
    use tokio::net::UnixStream;
    let mut stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| format!("Impossible de se connecter au socket '{}': {}", socket_path.display(), e))?;

    let mut payload = serde_json::to_string(req).map_err(|e| e.to_string())?;
    payload.push('\n');

    stream.write_all(payload.as_bytes()).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).await.map_err(|e| e.to_string())?;

    Ok(response.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unix_control_socket_and_integrity_audit() {
        let tmp_dir = std::env::temp_dir().join(format!("meshfs-sock-test-{}", std::process::id()));
        let sock_path = tmp_dir.join("control.sock");

        let engine = MeshFSEngine::new(&tmp_dir, "test-cluster", 8443).await.unwrap();

        // Write a test file into MeshFS
        let payload = b"Cryptographic sovereign storage test block data 2026";
        engine.put_file("default", "test-doc.txt", payload).await.unwrap();

        // Start Unix control socket
        start_control_socket(engine.clone(), sock_path.clone()).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 1. Query Ping
        let ping_resp = query_control_socket(&sock_path, &ControlRequest::Ping).await.unwrap();
        assert!(ping_resp.contains("pong"));

        // 2. Query Status
        let status_resp = query_control_socket(&sock_path, &ControlRequest::Status).await.unwrap();
        assert!(status_resp.contains("meshfs-distributed"));
        assert!(status_resp.contains("test-cluster"));

        // 3. Query Deep Integrity Inspect
        let inspect_resp = query_control_socket(
            &sock_path,
            &ControlRequest::Inspect {
                volume: None,
                deep_sha_verify: true,
            },
        )
        .await
        .unwrap();

        assert!(inspect_resp.contains("\"ok\":true"));
        assert!(inspect_resp.contains("Intégrité vérifiée à 100%"));

        // Cleanup
        let _ = fs::remove_dir_all(&tmp_dir);
    }
}

