//! PMBS Remote Sync Engine
//! Handles inter-site differential chunk replication (e.g. gbs-p4 <-> med-pmox).
//! Downloads only non-existent CAS chunks, minimizing inter-site WAN bandwidth.

use crate::datastore::{BackupManifest, Datastore};
use serde::{Deserialize, Serialize};

/// Report of an inter-site sync job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub ok: bool,
    #[serde(rename = "sourceRemote")]
    pub source_remote: String,
    #[serde(rename = "targetDatastore")]
    pub target_datastore: String,
    #[serde(rename = "snapshotsSynced")]
    pub snapshots_synced: usize,
    #[serde(rename = "chunksTransferred")]
    pub chunks_transferred: usize,
    #[serde(rename = "bytesTransferred")]
    pub bytes_transferred: u64,
    pub message: String,
}

/// Syncs snapshots from a remote PMBS server into a local Datastore.
pub async fn sync_from_remote(
    remote_url: &str,
    remote_datastore: &str,
    target_datastore: &Datastore,
) -> Result<SyncReport, String> {
    crate::datastore::validate_name(remote_datastore).map_err(|e| e.to_string())?;
    let client = crate::server::api_client();

    // 1. Fetch remote snapshots list
    let list_url = format!(
        "{}/api/v1/datastores/{}/snapshots",
        remote_url.trim_end_matches('/'),
        remote_datastore
    );

    let resp = client
        .get(&list_url)
        .send()
        .await
        .map_err(|e| format!("Connexion impossible au PMBS distant: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Erreur distante HTTP {}", resp.status()));
    }

    #[derive(Deserialize)]
    struct RemoteSnapshotsResp {
        snapshots: Vec<BackupManifest>,
    }

    let remote_data = resp
        .json::<RemoteSnapshotsResp>()
        .await
        .map_err(|e| format!("Payload distant invalide: {}", e))?;

    let mut snapshots_synced = 0;
    let mut chunks_transferred = 0;
    let mut bytes_transferred = 0u64;

    for snap in &remote_data.snapshots {
        // Collect missing chunks for this snapshot
        for (_file_path, file_meta) in &snap.files {
            for chunk_ref in &file_meta.chunks {
                if !target_datastore.has_chunk(&chunk_ref.id) {
                    // Download missing chunk
                    let chunk_url = format!(
                        "{}/api/v1/datastores/{}/chunks/{}",
                        remote_url.trim_end_matches('/'),
                        remote_datastore,
                        chunk_ref.id
                    );

                    let chunk_resp = client
                        .get(&chunk_url)
                        .send()
                        .await
                        .map_err(|e| format!("Erreur transfert chunk {}: {}", chunk_ref.id, e))?;

                    if chunk_resp.status().is_success() {
                        let bytes = chunk_resp
                            .bytes()
                            .await
                            .map_err(|e| format!("Erreur lecture octets chunk: {}", e))?;

                        target_datastore
                            .write_chunk(&bytes)
                            .map_err(|e| format!("Erreur écriture disque chunk: {}", e))?;

                        chunks_transferred += 1;
                        bytes_transferred += bytes.len() as u64;
                    }
                }
            }
        }

        // Save snapshot manifest locally
        target_datastore
            .save_snapshot(snap)
            .map_err(|e| format!("Impossible de sauvegarder le snapshot: {}", e))?;

        snapshots_synced += 1;
    }

    Ok(SyncReport {
        ok: true,
        source_remote: remote_url.to_string(),
        target_datastore: target_datastore.name.clone(),
        snapshots_synced,
        chunks_transferred,
        bytes_transferred,
        message: format!(
            "Synchronisation terminée: {} snapshots synchronisés, {} chunks transférés ({} octets).",
            snapshots_synced, chunks_transferred, bytes_transferred
        ),
    })
}
