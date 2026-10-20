//! MeshFS Engine — Core State and Concurrency Controller
//! Coordinates storage drivers, 4MB CAS chunking, Merkle DAG tracking,
//! volume isolation, and peer node synchronization.

use crate::chunk::{sha256_digest, ChunkRef, ChunkStore, CHUNK_SIZE};
use crate::cluster::{Action, Cluster, MAX_INLINE_BYTES};
use crate::merkle::Inode;
use crate::storage::{StorageDriver, StorageMetrics};
use crate::volume::{Snapshot, Volume};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;

/// Cluster node descriptor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeInfo {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub hostname: String,
    pub address: String,
    pub role: String,
    pub status: String,
    #[serde(rename = "joinedAt")]
    pub joined_at: String,
}

/// Persistent JSON state for MeshFS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineState {
    #[serde(rename = "clusterName")]
    pub cluster_name: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub hostname: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub nodes: Vec<NodeInfo>,
    pub volumes: HashMap<String, Volume>,
    pub snapshots: Vec<Snapshot>,
}

/// MeshFS overall status report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub ok: bool,
    pub service: String,
    pub cluster: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub hostname: String,
    #[serde(rename = "nodesCount")]
    pub nodes_count: usize,
    #[serde(rename = "volumesCount")]
    pub volumes_count: usize,
    #[serde(rename = "snapshotsCount")]
    pub snapshots_count: usize,
    #[serde(rename = "totalFiles")]
    pub total_files: usize,
    #[serde(rename = "totalLogicalBytes")]
    pub total_logical_bytes: u64,
    #[serde(rename = "diskTotal")]
    pub disk_total: u64,
    #[serde(rename = "diskUsed")]
    pub disk_used: u64,
    #[serde(rename = "diskFree")]
    pub disk_free: u64,
    #[serde(rename = "usedPercent")]
    pub used_percent: f64,
    #[serde(rename = "storageBackend")]
    pub storage_backend: String,
    #[serde(rename = "reflinkSupported")]
    pub reflink_supported: bool,
    #[serde(rename = "chunkStorePath")]
    pub chunk_store_path: String,
    #[serde(rename = "singleNodeAutonomous")]
    pub single_node_autonomous: bool,
    #[serde(rename = "supportsScaleOut")]
    pub supports_scale_out: bool,
    #[serde(rename = "compatibleWithoutPodmesh")]
    pub compatible_without_podmesh: bool,
    pub replication: Option<serde_json::Value>,
    pub namespace_consistency: String,
}

/// Main Thread-safe MeshFS Engine.
#[derive(Clone)]
pub struct MeshFSEngine {
    pub data_dir: PathBuf,
    pub meta_dir: PathBuf,
    pub state_file: PathBuf,
    pub chunk_store: ChunkStore,
    pub driver: StorageDriver,
    pub state: Arc<RwLock<EngineState>>,
    pub cluster: Arc<OnceLock<Arc<Cluster>>>,
}

impl MeshFSEngine {
    /// Initializes a new or existing MeshFS Engine instance.
    pub async fn new(data_dir: &Path, cluster_name: &str, listen_port: u16) -> io::Result<Self> {
        fs::create_dir_all(data_dir)?;
        let meta_dir = data_dir.join("meta");
        fs::create_dir_all(&meta_dir)?;

        let state_file = meta_dir.join("meshfs-state.json");
        let chunk_store = ChunkStore::new(data_dir)?;
        let driver = StorageDriver::detect(&chunk_store.chunks_dir);

        let initial_state = if state_file.exists() {
            serde_json::from_str::<EngineState>(&fs::read_to_string(&state_file)?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
        } else {
            Self::default_state(cluster_name, listen_port)
        };

        let engine = Self {
            data_dir: data_dir.to_path_buf(),
            meta_dir,
            state_file,
            chunk_store,
            driver,
            state: Arc::new(RwLock::new(initial_state)),
            cluster: Arc::new(OnceLock::new()),
        };

        // Ensure state is persisted
        engine.persist_state().await?;
        Ok(engine)
    }

    /// Generates standard initial state for a sovereign node.
    fn default_state(cluster_name: &str, port: u16) -> EngineState {
        let hostname = match std::env::var("HOSTNAME") {
            Ok(h) if !h.is_empty() => h,
            _ => {
                let mut buf = [0u8; 64];
                unsafe {
                    if libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) == 0 {
                        std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char)
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        "meshfs-node".to_string()
                    }
                }
            }
        };

        let now = Utc::now().to_rfc3339();
        let timestamp = Utc::now().timestamp();
        let node_id = format!("node-{}-{}", hostname, timestamp);

        let primary_node = NodeInfo {
            node_id: node_id.clone(),
            hostname: hostname.clone(),
            address: format!("127.0.0.1:{}", port),
            role: "primary".to_string(),
            status: "online".to_string(),
            joined_at: now.clone(),
        };

        let mut default_vol = Volume::new("default");
        default_vol.recompute_merkle();

        let mut volumes = HashMap::new();
        volumes.insert("default".to_string(), default_vol);

        EngineState {
            cluster_name: cluster_name.to_string(),
            node_id,
            hostname,
            created_at: now,
            nodes: vec![primary_node],
            volumes,
            snapshots: Vec::new(),
        }
    }

    /// Atomically writes state to disk (write to tmp file, then rename).
    pub async fn persist_state(&self) -> io::Result<()> {
        let state_guard = self.state.read().await;
        let serialized = serde_json::to_string_pretty(&*state_guard)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        use std::sync::atomic::{AtomicU64, Ordering};
        static STATE_TMP_COUNTER: AtomicU64 = AtomicU64::new(1);
        let seq = STATE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_file = self.meta_dir.join(format!(
            "meshfs-state.json.tmp.{}.{}.{}",
            std::process::id(),
            seq,
            nanos
        ));
        {
            let mut file = File::create(&tmp_file)?;
            file.write_all(serialized.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(tmp_file, &self.state_file)?;
        File::open(&self.meta_dir)?.sync_all()?;
        Ok(())
    }

    /// Retrieves comprehensive operational status.
    pub async fn get_status(&self) -> StatusReport {
        let cluster = self.cluster.get();
        let ready = if let Some(c) = cluster {
            c.namespace().await.is_ok()
        } else {
            true
        };
        let state = self.state.read().await;
        let metrics = self
            .driver
            .get_metrics(&self.data_dir)
            .unwrap_or(StorageMetrics {
                total_bytes: 0,
                used_bytes: 0,
                free_bytes: 0,
                used_percent: 0.0,
            });

        let total_files: usize = state.volumes.values().map(|v| v.files.len()).sum();
        let total_logical_bytes: u64 = state.volumes.values().map(|v| v.total_bytes()).sum();

        StatusReport {
            ok: ready,
            service: "meshfs-distributed".to_string(),
            cluster: state.cluster_name.clone(),
            node_id: state.node_id.clone(),
            hostname: state.hostname.clone(),
            nodes_count: cluster.map_or(state.nodes.len(), |c| c.config.peers.len()),
            volumes_count: state.volumes.len(),
            snapshots_count: state.snapshots.len(),
            total_files,
            total_logical_bytes,
            disk_total: metrics.total_bytes,
            disk_used: metrics.used_bytes,
            disk_free: metrics.free_bytes,
            used_percent: metrics.used_percent,
            storage_backend: self.driver.info.backend_type.clone(),
            reflink_supported: self.driver.info.features.reflink_support,
            chunk_store_path: self.chunk_store.chunks_dir.to_string_lossy().to_string(),
            single_node_autonomous: cluster.is_none(),
            supports_scale_out: cluster.is_some(),
            compatible_without_podmesh: true,
            replication: cluster.map(|c| c.metrics()),
            namespace_consistency: if cluster.is_some() {
                "diagnostic local counters; data reads require quorum"
            } else {
                "standalone"
            }
            .to_string(),
        }
    }

    /// Stores a file into a volume, splitting it into 4MB CAS chunks.
    pub async fn put_file(
        &self,
        volume_name: &str,
        file_path: &str,
        content: &[u8],
    ) -> io::Result<Inode> {
        if let Some(c) = self.cluster.get() {
            if content.len() > MAX_INLINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "clustered file limit is 16 MiB",
                ));
            }
            return serde_json::from_value(
                c.write(Action::Put {
                    volume: volume_name.into(),
                    path: file_path.into(),
                    hex: hex::encode(content),
                    at: Utc::now().to_rfc3339(),
                })
                .await?,
            )
            .map_err(io::Error::other);
        }
        let size = content.len() as u64;
        let file_digest = format!("sha256:{}", sha256_digest(content));
        let mut chunks = Vec::new();

        let mut offset = 0usize;
        while offset < content.len() {
            let end = (offset + CHUNK_SIZE).min(content.len());
            let slice = &content[offset..end];
            let chunk_id = self.chunk_store.write_chunk(slice)?;

            chunks.push(ChunkRef {
                id: chunk_id,
                size: slice.len(),
                offset: offset as u64,
            });

            offset = end;
        }

        let inode = Inode {
            revision: 0,
            path: file_path.to_string(),
            size,
            digest: file_digest,
            chunks_count: chunks.len(),
            chunks,
            updated_at: Utc::now().to_rfc3339(),
        };

        {
            let mut state = self.state.write().await;
            let vol = state
                .volumes
                .entry(volume_name.to_string())
                .or_insert_with(|| Volume::new(volume_name));
            vol.insert_file(inode.clone());
        }

        self.persist_state().await?;
        Ok(inode)
    }

    /// Stream bytes into bounded replicated blocks; publish the inode only after
    /// every block has committed. Interrupted uploads leave the old path intact.
    pub async fn put_stream<R: tokio::io::AsyncRead + Unpin + Send>(
        &self,
        volume: &str,
        path: &str,
        mut reader: R,
    ) -> io::Result<Inode> {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncReadExt;
        let mut buffer = vec![0; CHUNK_SIZE];
        let mut size = 0u64;
        let mut hash = Sha256::new();
        let mut chunks = Vec::new();
        let mut staged = std::collections::HashSet::new();
        loop {
            let mut used = 0;
            while used < buffer.len() {
                let n = reader.read(&mut buffer[used..]).await?;
                if n == 0 {
                    break;
                }
                used += n;
            }
            if used == 0 {
                break;
            }
            let bytes = &buffer[..used];
            hash.update(bytes);
            let digest = format!("sha256:{}", sha256_digest(bytes));
            let id = if let Some(c) = self.cluster.get() {
                if staged.contains(&digest) {
                    digest.clone()
                } else {
                    let id = serde_json::from_value::<String>(
                        c.write(Action::StageChunk {
                            hex: hex::encode(bytes),
                        })
                        .await?,
                    )
                    .map_err(io::Error::other)?;
                    if id != digest {
                        return Err(io::Error::other("committed chunk digest mismatch"));
                    }
                    staged.insert(id.clone());
                    id
                }
            } else {
                self.chunk_store.write_chunk(bytes)?
            };
            chunks.push(ChunkRef {
                id,
                size: used,
                offset: size,
            });
            size = size
                .checked_add(used as u64)
                .ok_or_else(|| io::Error::other("file size overflow"))?;
        }
        let inode = Inode {
            revision: 0,
            path: path.into(),
            size,
            digest: format!("sha256:{}", hex::encode(hash.finalize())),
            chunks_count: chunks.len(),
            chunks,
            updated_at: Utc::now().to_rfc3339(),
        };
        if let Some(c) = self.cluster.get() {
            return serde_json::from_value(
                c.write(Action::CommitFile {
                    volume: volume.into(),
                    inode,
                })
                .await?,
            )
            .map_err(io::Error::other);
        }
        {
            let mut s = self.state.write().await;
            s.volumes
                .entry(volume.into())
                .or_insert_with(|| Volume::new(volume))
                .insert_file(inode.clone());
        }
        self.persist_state().await?;
        Ok(inode)
    }
    pub async fn local_inode(&self, volume: &str, path: &str) -> io::Result<Inode> {
        self.state
            .read()
            .await
            .volumes
            .get(volume)
            .and_then(|v| v.files.get(path))
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
    }
    pub async fn file_inode(&self, volume: &str, path: &str) -> io::Result<Inode> {
        if let Some(c) = self.cluster.get() {
            c.inode(volume, path).await
        } else {
            self.local_inode(volume, path).await
        }
    }
    pub async fn file_chunk(&self, id: &str) -> io::Result<Vec<u8>> {
        if let Some(c) = self.cluster.get() {
            c.chunk(id).await
        } else {
            self.chunk_store.read_chunk(id)
        }
    }
    /// Reassembles a file from its CAS chunks.
    pub async fn get_file(&self, volume_name: &str, file_path: &str) -> io::Result<Vec<u8>> {
        if let Some(c) = self.cluster.get() {
            return c.read(volume_name, file_path).await;
        }
        self.local_get_file(volume_name, file_path).await
    }

    pub(crate) async fn local_get_file(
        &self,
        volume_name: &str,
        file_path: &str,
    ) -> io::Result<Vec<u8>> {
        let state = self.state.read().await;
        let vol = state.volumes.get(volume_name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Volume '{}' introuvable", volume_name),
            )
        })?;

        let inode = vol.get_file(file_path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Fichier '{}:{}' introuvable", volume_name, file_path),
            )
        })?;

        let mut buffer = Vec::with_capacity(inode.size as usize);
        for chunk_ref in &inode.chunks {
            let data = self.chunk_store.read_chunk(&chunk_ref.id)?;
            buffer.extend_from_slice(&data);
        }

        if buffer.len() as u64 != inode.size
            || format!("sha256:{}", sha256_digest(&buffer)) != inode.digest
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file size or digest mismatch",
            ));
        }
        Ok(buffer)
    }

    /// Creates a new logical volume.
    pub async fn create_volume(&self, name: &str) -> io::Result<Volume> {
        if let Some(c) = self.cluster.get() {
            return serde_json::from_value(
                c.write(Action::CreateVolume {
                    name: name.into(),
                    at: Utc::now().to_rfc3339(),
                })
                .await?,
            )
            .map_err(io::Error::other);
        }
        let vol = {
            let mut state = self.state.write().await;
            if let Some(existing) = state.volumes.get(name) {
                return Ok(existing.clone());
            }
            let new_vol = Volume::new(name);
            state.volumes.insert(name.to_string(), new_vol.clone());
            new_vol
        };

        self.persist_state().await?;
        Ok(vol)
    }

    /// Deletes a logical volume.
    pub async fn delete_volume(&self, name: &str) -> io::Result<bool> {
        if let Some(c) = self.cluster.get() {
            return serde_json::from_value(
                c.write(Action::DeleteVolume { name: name.into() }).await?,
            )
            .map_err(io::Error::other);
        }
        let removed = {
            let mut state = self.state.write().await;
            state.volumes.remove(name).is_some()
        };

        if removed {
            self.persist_state().await?;
        }
        Ok(removed)
    }

    /// Lists all volumes.
    pub async fn list_volumes(&self) -> io::Result<HashMap<String, Volume>> {
        if let Some(c) = self.cluster.get() {
            return serde_json::from_value(c.namespace().await?["volumes"].clone())
                .map_err(io::Error::other);
        }
        Ok(self.state.read().await.volumes.clone())
    }

    /// Creates an instant O(1) Copy-on-Write Merkle snapshot.
    pub async fn create_snapshot(
        &self,
        volume_name: &str,
        custom_name: Option<&str>,
    ) -> io::Result<Snapshot> {
        if let Some(c) = self.cluster.get() {
            let id = format!(
                "snapshot-{}",
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            );
            return serde_json::from_value(
                c.write(Action::Snapshot {
                    volume: volume_name.into(),
                    name: custom_name.unwrap_or(&id).into(),
                    id: id.clone(),
                    at: Utc::now().to_rfc3339(),
                })
                .await?,
            )
            .map_err(io::Error::other);
        }
        let snap = {
            let mut state = self.state.write().await;
            let vol = state.volumes.get_mut(volume_name).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Volume '{}' introuvable", volume_name),
                )
            })?;

            // Ensure Merkle root is strictly fresh
            vol.recompute_merkle();
            let snapshot = Snapshot::create_from_volume(vol, custom_name);
            state.snapshots.insert(0, snapshot.clone());
            snapshot
        };

        self.persist_state().await?;
        Ok(snap)
    }

    /// Deletes a snapshot by name or ID.
    pub async fn delete_snapshot(
        &self,
        volume_name: &str,
        snapshot_name: &str,
    ) -> io::Result<bool> {
        if let Some(c) = self.cluster.get() {
            return serde_json::from_value(
                c.write(Action::DeleteSnapshot {
                    volume: volume_name.into(),
                    name: snapshot_name.into(),
                })
                .await?,
            )
            .map_err(io::Error::other);
        }
        let removed = {
            let mut state = self.state.write().await;
            let initial_len = state.snapshots.len();
            state.snapshots.retain(|s| {
                !(s.volume == volume_name && (s.name == snapshot_name || s.id == snapshot_name))
            });
            state.snapshots.len() < initial_len
        };

        if removed {
            self.persist_state().await?;
        }
        Ok(removed)
    }

    /// Lists snapshots, optionally filtered by volume.
    pub async fn list_snapshots(&self, volume_filter: Option<&str>) -> io::Result<Vec<Snapshot>> {
        let snapshots: Vec<Snapshot> = if let Some(c) = self.cluster.get() {
            serde_json::from_value(c.namespace().await?["snapshots"].clone())
                .map_err(io::Error::other)?
        } else {
            self.state.read().await.snapshots.clone()
        };
        Ok(snapshots
            .into_iter()
            .filter(|s| volume_filter.is_none_or(|v| s.volume == v))
            .collect())
    }

    /// Joins an existing MeshFS cluster node and exchanges node peer records.
    pub async fn join_cluster(
        &self,
        target_address: &str,
        _token: &str,
    ) -> Result<Vec<NodeInfo>, String> {
        let c = self.cluster.get().ok_or(
            "peer discovery alone does not replicate data; configure a fixed cluster explicitly",
        )?;
        let target = target_address.trim_end_matches('/');
        if !c
            .config
            .peers
            .values()
            .any(|p| p.trim_end_matches('/') == target)
        {
            return Err("target is not in the configured membership".into());
        }
        c.namespace().await.map_err(|e| e.to_string())?;
        Ok(c.nodes().await)
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    #[tokio::test]
    async fn large_stream_and_interrupted_replacement() {
        let dir = std::env::temp_dir().join(format!("meshfs-stream-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let engine = MeshFSEngine::new(&dir, "test", 0).await.unwrap();
        let length = 20 * 1024 * 1024 + 31;
        let inode = engine
            .put_stream("default", "disk.raw", tokio::io::repeat(0x5a).take(length))
            .await
            .unwrap();
        assert_eq!(inode.size, length);
        assert_eq!(inode.chunks.len(), 6);
        let partial = futures_util::stream::iter(vec![
            Ok(bytes::Bytes::from(vec![7u8; CHUNK_SIZE])),
            Err(std::io::Error::other("source disconnected")),
        ]);
        assert!(engine
            .put_stream(
                "default",
                "disk.raw",
                tokio_util::io::StreamReader::new(partial)
            )
            .await
            .is_err());
        assert_eq!(
            engine.local_inode("default", "disk.raw").await.unwrap(),
            inode
        );
        let restored = engine.local_get_file("default", "disk.raw").await.unwrap();
        assert_eq!(restored.len(), length as usize);
        assert!(restored.iter().all(|b| *b == 0x5a));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
