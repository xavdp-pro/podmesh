//! MeshFS Engine — Core State and Concurrency Controller
//! Coordinates storage drivers, 4MB CAS chunking, Merkle DAG tracking,
//! volume isolation, and peer node synchronization.

use crate::chunk::{sha256_digest, ChunkRef, ChunkStore, CHUNK_SIZE};
use crate::merkle::Inode;
use crate::storage::{StorageDriver, StorageMetrics};
use crate::volume::{Snapshot, Volume};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
            match fs::read_to_string(&state_file) {
                Ok(content) => serde_json::from_str::<EngineState>(&content)
                    .unwrap_or_else(|_| Self::default_state(cluster_name, listen_port)),
                Err(_) => Self::default_state(cluster_name, listen_port),
            }
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
            node_id: format!("node-{}", hostname),
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
        drop(state_guard);

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
        Ok(())
    }

    /// Retrieves comprehensive operational status.
    pub async fn get_status(&self) -> StatusReport {
        let state = self.state.read().await;
        let metrics = self.driver.get_metrics(&self.data_dir).unwrap_or(StorageMetrics {
            total_bytes: 0,
            used_bytes: 0,
            free_bytes: 0,
            used_percent: 0.0,
        });

        let total_files: usize = state.volumes.values().map(|v| v.files.len()).sum();
        let total_logical_bytes: u64 = state.volumes.values().map(|v| v.total_bytes()).sum();

        StatusReport {
            ok: true,
            service: "meshfs-distributed".to_string(),
            cluster: state.cluster_name.clone(),
            node_id: state.node_id.clone(),
            hostname: state.hostname.clone(),
            nodes_count: state.nodes.len(),
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
            single_node_autonomous: true,
            supports_scale_out: true,
            compatible_without_podmesh: true,
        }
    }

    /// Stores a file into a volume, splitting it into 4MB CAS chunks.
    pub async fn put_file(&self, volume_name: &str, file_path: &str, content: &[u8]) -> io::Result<Inode> {
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
            path: file_path.to_string(),
            size,
            digest: file_digest,
            chunks_count: chunks.len(),
            chunks,
            updated_at: Utc::now().to_rfc3339(),
        };

        {
            let mut state = self.state.write().await;
            let vol = state.volumes.entry(volume_name.to_string()).or_insert_with(|| Volume::new(volume_name));
            vol.insert_file(inode.clone());
        }

        self.persist_state().await?;
        Ok(inode)
    }

    /// Reassembles a file from its CAS chunks.
    pub async fn get_file(&self, volume_name: &str, file_path: &str) -> io::Result<Vec<u8>> {
        let state = self.state.read().await;
        let vol = state.volumes.get(volume_name).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("Volume '{}' introuvable", volume_name))
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

        Ok(buffer)
    }

    /// Creates a new logical volume.
    pub async fn create_volume(&self, name: &str) -> io::Result<Volume> {
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
    pub async fn list_volumes(&self) -> HashMap<String, Volume> {
        self.state.read().await.volumes.clone()
    }

    /// Creates an instant O(1) Copy-on-Write Merkle snapshot.
    pub async fn create_snapshot(&self, volume_name: &str, custom_name: Option<&str>) -> io::Result<Snapshot> {
        let snap = {
            let mut state = self.state.write().await;
            let vol = state.volumes.get_mut(volume_name).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("Volume '{}' introuvable", volume_name))
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
    pub async fn delete_snapshot(&self, volume_name: &str, snapshot_name: &str) -> io::Result<bool> {
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
    pub async fn list_snapshots(&self, volume_filter: Option<&str>) -> Vec<Snapshot> {
        let state = self.state.read().await;
        match volume_filter {
            Some(v) => state.snapshots.iter().filter(|s| s.volume == v).cloned().collect(),
            None => state.snapshots.clone(),
        }
    }

    /// Joins an existing MeshFS cluster node and exchanges node peer records.
    pub async fn join_cluster(&self, target_address: &str, _token: &str) -> Result<Vec<NodeInfo>, String> {
        let url = if target_address.starts_with("http://") || target_address.starts_with("https://") {
            format!("{}/api/v1/meshfs/nodes", target_address)
        } else {
            format!("http://{}/api/v1/meshfs/nodes", target_address)
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| e.to_string())?;

        let response = client.get(&url).send().await.map_err(|e| format!("Connexion échouée: {}", e))?;
        if !response.status().is_success() {
            return Err(format!("Statut distant non-OK: {}", response.status()));
        }

        #[derive(Deserialize)]
        struct RemoteNodesResponse {
            nodes: Vec<NodeInfo>,
        }

        let data = response
            .json::<RemoteNodesResponse>()
            .await
            .map_err(|e| format!("Payload distant invalide: {}", e))?;

        {
            let mut state = self.state.write().await;
            let mut known_ids: std::collections::HashSet<String> =
                state.nodes.iter().map(|n| n.node_id.clone()).collect();

            for remote_node in data.nodes {
                if !known_ids.contains(&remote_node.node_id) {
                    known_ids.insert(remote_node.node_id.clone());
                    state.nodes.push(remote_node);
                }
            }
        }

        let _ = self.persist_state().await;
        Ok(self.state.read().await.nodes.clone())
    }
}
