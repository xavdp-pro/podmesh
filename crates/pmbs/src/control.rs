//! PMBS Unix Domain Socket Control IPC & Local Diagnostics

use crate::server::AppState;
use crate::verify::verify_snapshot;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum ControlRequest {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "verify")]
    Verify {
        #[serde(default)]
        datastore: Option<String>,
        #[serde(default)]
        snapshot_id: Option<String>,
    },
}

pub async fn start_control_socket(state: AppState, socket_path: PathBuf) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    info!(path = %socket_path.display(), "PMBS Control Unix Socket actif");

    let state = Arc::new(state);

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let state = Arc::clone(&state);
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

                            debug!(raw = %trimmed, "Commande reçue sur le socket de contrôle PMBS");

                            let response = match serde_json::from_str::<ControlRequest>(trimmed) {
                                Ok(ControlRequest::Ping) => serde_json::json!({
                                    "status": "pong",
                                    "service": "pmbs-pure-rust",
                                    "timestamp": chrono::Utc::now().to_rfc3339()
                                }),
                                Ok(ControlRequest::Status) => {
                                    let stores = state.datastores.read().await;
                                    let names: Vec<String> = stores.keys().cloned().collect();
                                    serde_json::json!({
                                        "ok": true,
                                        "service": "podmesh-backup-server",
                                        "version": "2.4.1",
                                        "datastores": names,
                                        "dedupEngine": "SHA-256 CAS 4MB (Pure Rust)",
                                        "op10VerifierReady": false,
                                        "remoteSyncEnabled": false
                                    })
                                }
                                Ok(ControlRequest::Verify { datastore, snapshot_id }) => {
                                    let stores = state.datastores.read().await;
                                    let target_name = datastore.unwrap_or_else(|| "default".to_string());
                                    if let Some(ds) = stores.get(&target_name) {
                                        let snaps = ds.list_snapshots();
                                        let mut reports = Vec::new();
                                        for snap in &snaps {
                                            if let Some(ref tid) = snapshot_id {
                                                if &snap.id != tid { continue; }
                                            }
                                            reports.push(verify_snapshot(ds, snap));
                                        }
                                        serde_json::json!({
                                            "ok": true,
                                            "datastore": target_name,
                                            "reports": reports
                                        })
                                    } else {
                                        serde_json::json!({ "ok": false, "error": "Datastore introuvable" })
                                    }
                                }
                                Err(_) => {
                                    match trimmed.to_lowercase().as_str() {
                                        "ping" => serde_json::json!({ "status": "pong" }),
                                        "status" => {
                                            let stores = state.datastores.read().await;
                                            serde_json::json!({
                                                "ok": true,
                                                "datastores": stores.keys().cloned().collect::<Vec<_>>()
                                            })
                                        }
                                        _ => serde_json::json!({
                                            "ok": false,
                                            "error": format!("Commande inconnue: '{}'. Actions: ping, status, verify", trimmed)
                                        }),
                                    }
                                }
                            };

                            let mut reply = serde_json::to_string(&response).unwrap_or_default();
                            reply.push('\n');
                            if let Err(e) = writer.write_all(reply.as_bytes()).await {
                                error!(err = %e, "Erreur écriture socket de contrôle PMBS");
                                break;
                            }
                            let _ = writer.flush().await;
                            line.clear();
                        }
                    });
                }
                Err(e) => {
                    warn!(err = %e, "Erreur acceptation socket de contrôle Unix PMBS");
                }
            }
        }
    });

    Ok(())
}

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
    use crate::datastore::Datastore;
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn test_pmbs_unix_control_socket() {
        let tmp_dir = std::env::temp_dir().join(format!("pmbs-sock-test-{}", std::process::id()));
        let sock_path = tmp_dir.join("pmbs-control.sock");

        let ds = Datastore::new(&tmp_dir, "datastore-default").unwrap();
        let mut map = HashMap::new();
        map.insert("datastore-default".to_string(), ds);

        let state = AppState {
            base_dir: tmp_dir.clone(),
            datastores: Arc::new(RwLock::new(map)),
        };

        start_control_socket(state, sock_path.clone()).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Query Ping
        let ping = query_control_socket(&sock_path, &ControlRequest::Ping).await.unwrap();
        assert!(ping.contains("pong"));

        // Query Status
        let status = query_control_socket(&sock_path, &ControlRequest::Status).await.unwrap();
        assert!(status.contains("podmesh-backup-server"));
        assert!(status.contains("datastore-default"));

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}

