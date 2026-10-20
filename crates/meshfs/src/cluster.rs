//! Quorum-committed MeshFS namespaces. Raft orders mutations; SQLite durably stores
//! votes, logs and applied state. A returning peer catches up through log replay
//! or a snapshot containing both metadata and immutable CAS content.
use crate::{
    chunk::{sha256_digest, ChunkRef, CHUNK_SIZE},
    engine::MeshFSEngine,
    merkle::Inode,
    volume::{Snapshot as FileSnapshot, Volume},
};
use openraft::error::{InstallSnapshotError, NetworkError, RPCError, RaftError, RemoteError};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::*;
use openraft::storage::{LogFlushed, RaftLogStorage, RaftStateMachine};
use openraft::{
    BasicNode, Entry, EntryPayload, LogId, LogState, RaftLogReader, RaftSnapshotBuilder,
    RaftTypeConfig, Snapshot, SnapshotMeta, StorageError, StorageIOError, StoredMembership, Vote,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Debug,
    io::{self, Read, Write, Seek, SeekFrom},
    ops::RangeBounds,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;

pub const MAX_INLINE_BYTES: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    /// Replicate one durable immutable block before publishing file metadata.
    StageChunk { hex: String },
    CommitFile { volume: String, inode: Inode },
    Put {
        volume: String,
        path: String,
        hex: String,
        at: String,
    },
    CreateVolume {
        name: String,
        at: String,
    },
    DeleteVolume {
        name: String,
    },
    Snapshot {
        volume: String,
        name: String,
        id: String,
        at: String,
    },
    DeleteSnapshot {
        volume: String,
        name: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub action: Action,
}
openraft::declare_raft_types!(pub Types: D = Command, R = Value, SnapshotData = tokio::fs::File);
pub type Raft = openraft::Raft<Types>;
type SResult<T> = Result<T, StorageError<u64>>;
type RpcErr<E = openraft::error::Infallible> = RPCError<u64, BasicNode, RaftError<u64, E>>;
fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
fn se(e: impl std::fmt::Display) -> StorageError<u64> {
    StorageIOError::write_state_machine(&err(e)).into()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub cluster: String,
    pub node_id: u64,
    pub peers: BTreeMap<u64, String>,
}
impl ClusterConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        let c: Self = serde_json::from_slice(&std::fs::read(path)?).map_err(err)?;
        if c.cluster.is_empty()
            || !c.peers.contains_key(&c.node_id)
            || c.peers.len() < 3
            || c.peers.len() % 2 == 0
        {
            return Err(err("cluster requires an odd fixed membership of at least three nodes, including this node"));
        }
        let mut urls = std::collections::HashSet::new();
        for url in c.peers.values() {
            let u = reqwest::Url::parse(url).map_err(err)?;
            if !matches!(u.scheme(), "http" | "https")
                || u.host_str().is_none()
                || u.path() != "/"
                || u.query().is_some()
                || u.fragment().is_some()
                || !u.username().is_empty()
                || u.password().is_some()
                || !urls.insert(url.trim_end_matches('/').to_string())
            {
                return Err(err("invalid or duplicate peer URL"));
            }
        }
        Ok(c)
    }
    fn fingerprint(&self) -> String {
        sha256_digest(
            &serde_json::to_vec(&("meshfs-stream-v1", &self.cluster, &self.peers)).expect("serializable topology"),
        )
    }
    fn nodes(&self) -> BTreeMap<u64, BasicNode> {
        self.peers
            .iter()
            .map(|(id, url)| (*id, BasicNode::new(url.clone())))
            .collect()
    }
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}
impl Db {
    fn open(path: &Path) -> io::Result<Self> {
        let c = Connection::open(path).map_err(err)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY,v TEXT NOT NULL); CREATE TABLE IF NOT EXISTS logs(idx INTEGER PRIMARY KEY,entry TEXT NOT NULL);").map_err(err)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(c)),
        })
    }
    fn get<T: DeserializeOwned>(&self, k: &str) -> SResult<Option<T>> {
        let c = self.conn.lock().map_err(se)?;
        let s: Option<String> = c
            .query_row("SELECT v FROM meta WHERE k=?", [k], |r| r.get(0))
            .optional()
            .map_err(se)?;
        s.map(|s| serde_json::from_str(&s).map_err(se)).transpose()
    }
    fn set<T: Serialize>(&self, k: &str, v: &T) -> SResult<()> {
        self.conn
            .lock()
            .map_err(se)?
            .execute(
                "INSERT INTO meta(k,v) VALUES(?,?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                params![k, serde_json::to_string(v).map_err(se)?],
            )
            .map_err(se)?;
        Ok(())
    }
}
impl RaftLogReader<Types> for Db {
    async fn try_get_log_entries<R: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: R,
    ) -> SResult<Vec<Entry<Types>>> {
        let c = self.conn.lock().map_err(se)?;
        let mut q = c
            .prepare("SELECT idx,entry FROM logs ORDER BY idx")
            .map_err(se)?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)))
            .map_err(se)?;
        let mut out = Vec::new();
        for row in rows {
            let (i, s) = row.map_err(se)?;
            if range.contains(&i) {
                out.push(serde_json::from_str(&s).map_err(se)?);
            }
        }
        Ok(out)
    }
}
impl RaftLogStorage<Types> for Db {
    type LogReader = Self;
    async fn get_log_state(&mut self) -> SResult<LogState<Types>> {
        let purged: Option<LogId<u64>> = self.get("purged")?;
        let c = self.conn.lock().map_err(se)?;
        let row: Option<String> = c
            .query_row(
                "SELECT entry FROM logs ORDER BY idx DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(se)?;
        let last = match row {
            Some(s) => Some(serde_json::from_str::<Entry<Types>>(&s).map_err(se)?.log_id),
            None => purged,
        };
        Ok(LogState {
            last_purged_log_id: purged,
            last_log_id: last,
        })
    }
    async fn get_log_reader(&mut self) -> Self {
        self.clone()
    }
    async fn save_vote(&mut self, v: &Vote<u64>) -> SResult<()> {
        self.set("vote", v)
    }
    async fn read_vote(&mut self) -> SResult<Option<Vote<u64>>> {
        self.get("vote")
    }
    async fn save_committed(&mut self, v: Option<LogId<u64>>) -> SResult<()> {
        self.set("committed", &v)
    }
    async fn read_committed(&mut self) -> SResult<Option<LogId<u64>>> {
        Ok(self.get::<Option<LogId<u64>>>("committed")?.flatten())
    }
    async fn append<I>(&mut self, entries: I, callback: LogFlushed<Types>) -> SResult<()>
    where
        I: IntoIterator<Item = Entry<Types>> + Send,
        I::IntoIter: Send,
    {
        {
            let mut c = self.conn.lock().map_err(se)?;
            let tx = c.transaction().map_err(se)?;
            for e in entries {
                tx.execute("INSERT INTO logs(idx,entry) VALUES(?,?) ON CONFLICT(idx) DO UPDATE SET entry=excluded.entry",params![e.log_id.index,serde_json::to_string(&e).map_err(se)?]).map_err(se)?;
            }
            tx.commit().map_err(se)?;
        }
        callback.log_io_completed(Ok(()));
        Ok(())
    }
    async fn truncate(&mut self, id: LogId<u64>) -> SResult<()> {
        self.conn
            .lock()
            .map_err(se)?
            .execute("DELETE FROM logs WHERE idx>=?", [id.index])
            .map_err(se)?;
        Ok(())
    }
    async fn purge(&mut self, id: LogId<u64>) -> SResult<()> {
        let mut c = self.conn.lock().map_err(se)?;
        let tx = c.transaction().map_err(se)?;
        tx.execute("DELETE FROM logs WHERE idx<=?", [id.index])
            .map_err(se)?;
        tx.execute(
            "INSERT INTO meta(k,v) VALUES('purged',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            [serde_json::to_string(&id).map_err(se)?],
        )
        .map_err(se)?;
        tx.commit().map_err(se)?;
        Ok(())
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct MachineData {
    applied: Option<LogId<u64>>,
    membership: StoredMembership<u64, BasicNode>,
    volumes: HashMap<String, Volume>,
    snapshots: Vec<FileSnapshot>,
    seen: BTreeMap<String, (String, Value)>,
    // Staged blocks remain referenced even if an upload is interrupted. No GC.
    #[serde(default)]
    blobs: BTreeMap<String,usize>,
}
pub struct Machine {
    db: Db,
    engine: MeshFSEngine,
    data: AsyncMutex<MachineData>,
}
impl Machine {
    async fn mirror(&self, data: &MachineData) {
        let mut s = self.engine.state.write().await;
        s.volumes = data.volumes.clone();
        s.snapshots = data.snapshots.clone();
    }
    fn execute(&self, s: &mut MachineData, c: &Command) -> SResult<Value> {
        let digest = sha256_digest(&serde_json::to_vec(&c.action).map_err(se)?);
        if let Some((previous, result)) = s.seen.get(&c.id) {
            return Ok(if previous == &digest {
                result.clone()
            } else {
                json!({"error":"operation ID reused for different content"})
            });
        }
        let result = match &c.action {
            Action::StageChunk { hex } => {
                let bytes=hex::decode(hex).map_err(se)?;
                if bytes.len()>CHUNK_SIZE {return Err(se("oversized staged chunk"));}
                let id=self.engine.chunk_store.write_chunk(&bytes).map_err(se)?;
                s.blobs.insert(id.clone(),bytes.len()); json!(id)
            },
            Action::CommitFile {volume,inode} => {
                validate_layout(inode,&s.blobs)?;
                let v=s.volumes.entry(volume.clone()).or_insert_with(|| {
                    let mut v=Volume::new(volume);v.created_at=inode.updated_at.clone();v
                });
                v.insert_file(inode.clone());json!(inode)
            },
            Action::Put {
                volume,
                path,
                hex,
                at,
            } => {
                let bytes = hex::decode(hex).map_err(se)?;
                let mut chunks = Vec::new();
                let mut offset = 0;
                for part in bytes.chunks(CHUNK_SIZE) {
                    let id = self.engine.chunk_store.write_chunk(part).map_err(se)?;
                    s.blobs.insert(id.clone(),part.len());
                    chunks.push(ChunkRef {
                        id,
                        size: part.len(),
                        offset,
                    });
                    offset += part.len() as u64;
                }
                let inode = Inode {
                    path: path.clone(),
                    size: bytes.len() as u64,
                    digest: format!("sha256:{}", sha256_digest(&bytes)),
                    chunks_count: chunks.len(),
                    chunks,
                    updated_at: at.clone(),
                };
                let vol = s.volumes.entry(volume.clone()).or_insert_with(|| {
                    let mut v = Volume::new(volume);
                    v.created_at = at.clone();
                    v
                });
                vol.insert_file(inode.clone());
                json!(inode)
            }
            Action::CreateVolume { name, at } => {
                let v = s.volumes.entry(name.clone()).or_insert_with(|| {
                    let mut v = Volume::new(name);
                    v.created_at = at.clone();
                    v
                });
                json!(v)
            }
            Action::DeleteVolume { name } => json!(s.volumes.remove(name).is_some()),
            Action::Snapshot {
                volume,
                name,
                id,
                at,
            } => match s.volumes.get(volume) {
                None => json!({"error":"volume not found"}),
                Some(v) => {
                    let snap = FileSnapshot {
                        id: id.clone(),
                        name: name.clone(),
                        volume: volume.clone(),
                        merkle_root: v.merkle_root.clone(),
                        files_count: v.files.len(),
                        total_bytes: v.total_bytes(),
                        created_at: at.clone(),
                        files_tree: v.files.clone(),
                    };
                    s.snapshots.insert(0, snap.clone());
                    json!(snap)
                }
            },
            Action::DeleteSnapshot { volume, name } => {
                let len = s.snapshots.len();
                s.snapshots
                    .retain(|v| !(v.volume == *volume && (v.name == *name || v.id == *name)));
                json!(len != s.snapshots.len())
            }
        };
        s.seen.insert(c.id.clone(), (digest, result.clone()));
        Ok(result)
    }
    fn snapshot_file(&self)->SResult<std::fs::File> {
        let dir=self.engine.data_dir.join("raft/snapshots");std::fs::create_dir_all(&dir).map_err(se)?;
        static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
        let path=dir.join(format!("transfer-{}-{}",std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
        let file=std::fs::OpenOptions::new().create_new(true).read(true).write(true).open(&path).map_err(se)?;
        // OpenRaft owns the descriptor until transfer completion. Unlinking the
        // temporary name also removes abandoned transfers on process termination.
        std::fs::remove_file(path).map_err(se)?;Ok(file)
    }
    fn chunk_ids(s:&MachineData)->BTreeSet<String> {
        let mut ids:BTreeSet<String>=s.blobs.keys().cloned().collect();
        for inode in s.volumes.values().flat_map(|v|v.files.values()).chain(s.snapshots.iter().flat_map(|v|v.files_tree.values())) {
            for chunk in &inode.chunks {ids.insert(chunk.id.clone());}
        }
        ids
    }

}
impl RaftStateMachine<Types> for Arc<Machine> {
    type SnapshotBuilder = Self;
    async fn applied_state(
        &mut self,
    ) -> SResult<(Option<LogId<u64>>, StoredMembership<u64, BasicNode>)> {
        let s = self.data.lock().await;
        Ok((s.applied, s.membership.clone()))
    }
    async fn apply<I>(&mut self, entries: I) -> SResult<Vec<Value>>
    where
        I: IntoIterator<Item = Entry<Types>> + Send,
        I::IntoIter: Send,
    {
        let entries: Vec<_> = entries.into_iter().collect();
        let mut s = self.data.lock().await;
        let mut out = Vec::new();
        for e in entries {
            let value = match e.payload {
                EntryPayload::Blank => Value::Null,
                EntryPayload::Membership(m) => {
                    s.membership = StoredMembership::new(Some(e.log_id), m);
                    Value::Null
                }
                EntryPayload::Normal(c) => self.execute(&mut s, &c)?,
            };
            s.applied = Some(e.log_id);
            out.push(value);
        }
        self.db.set("machine", &*s)?;
        self.mirror(&s).await;
        Ok(out)
    }
    async fn get_snapshot_builder(&mut self) -> Self {
        self.clone()
    }
    async fn begin_receiving_snapshot(
        &mut self,
    ) -> SResult<Box<<Types as RaftTypeConfig>::SnapshotData>> {
        Ok(Box::new(tokio::fs::File::from_std(self.snapshot_file()?)))
    }
    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, BasicNode>,
        snapshot: Box<<Types as RaftTypeConfig>::SnapshotData>,
    ) -> SResult<()> {
        let mut file=(*snapshot).into_std().await;
        file.sync_all().map_err(se)?;file.seek(SeekFrom::Start(0)).map_err(se)?;
        let mut reader=std::io::BufReader::new(file);
        let mut checksum=[0u8;32];reader.read_exact(&mut checksum).map_err(se)?;
        let metadata=read_frame(&mut reader,128*1024*1024)?;
        if hex::encode(checksum)!=sha256_digest(&metadata){return Err(se("snapshot metadata checksum mismatch"));}
        let state:MachineData=serde_json::from_slice(&metadata).map_err(se)?;
        if state.applied!=meta.last_log_id || state.membership!=meta.last_membership {return Err(se("snapshot metadata mismatch"));}
        let expected=Machine::chunk_ids(&state);
        let mut sizes=BTreeMap::new();
        for id in &expected {
            let bytes=read_frame(&mut reader,CHUNK_SIZE)?;
            sizes.insert(id.clone(),bytes.len());
            if self.engine.chunk_store.write_chunk(&bytes).map_err(se)?!=*id {return Err(se("snapshot chunk hash mismatch"));}
        }
        let mut extra=[0u8;1];if reader.read(&mut extra).map_err(se)?!=0{return Err(se("snapshot trailing bytes"));}
        for inode in state.volumes.values().flat_map(|v|v.files.values()).chain(state.snapshots.iter().flat_map(|v|v.files_tree.values())) {validate_layout(inode,&sizes)?;}
        let mut s=self.data.lock().await;self.db.set("machine",&state)?;*s=state;self.mirror(&s).await;
        Ok(())
    }
    async fn get_current_snapshot(&mut self) -> SResult<Option<Snapshot<Types>>> {
        if self.data.lock().await.applied.is_none() {
            return Ok(None);
        }
        self.build_snapshot().await.map(Some)
    }
}
impl RaftSnapshotBuilder<Types> for Arc<Machine> {
    async fn build_snapshot(&mut self) -> SResult<Snapshot<Types>> {
        let state=self.data.lock().await.clone();let machine=self.clone();
        tokio::task::spawn_blocking(move|| {
            let meta=SnapshotMeta{last_log_id:state.applied,last_membership:state.membership.clone(),snapshot_id:format!("{:?}",state.applied)};
            let mut file=machine.snapshot_file()?;
            let metadata=serde_json::to_vec(&state).map_err(se)?;
            file.write_all(&hex::decode(sha256_digest(&metadata)).map_err(se)?).map_err(se)?;
            write_frame(&mut file,&metadata)?;
            for id in Machine::chunk_ids(&state){write_frame(&mut file,&machine.engine.chunk_store.read_chunk(&id).map_err(se)?)?;}
            file.sync_all().map_err(se)?;file.seek(SeekFrom::Start(0)).map_err(se)?;
            Ok(Snapshot{meta,snapshot:Box::new(tokio::fs::File::from_std(file))})
        }).await.map_err(se)?
    }
}

#[derive(Clone)]
pub struct Network {
    client: reqwest::Client,
    token: Arc<str>,
    fingerprint: String,
}
pub struct Peer {
    network: Network,
    target: u64,
    node: BasicNode,
}
impl Network {
    async fn rpc<Q: Serialize, R: DeserializeOwned, E: std::error::Error + DeserializeOwned>(
        &self,
        target: u64,
        node: &BasicNode,
        path: &str,
        req: Q,
    ) -> Result<R, RPCError<u64, BasicNode, E>> {
        let response = self
            .client
            .post(format!(
                "{}/internal/{}",
                node.addr.trim_end_matches('/'),
                path
            ))
            .bearer_auth(&*self.token)
            .header("x-meshfs-topology", &self.fingerprint)
            .json(&req)
            .timeout(std::time::Duration::from_secs(if path=="snapshot" {120}else{4}))
            .send()
            .await
            .map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
        let result: Result<R, E> = response
            .error_for_status()
            .map_err(|e| RPCError::Network(NetworkError::new(&e)))?
            .json()
            .await
            .map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
        result.map_err(|e| RPCError::RemoteError(RemoteError::new(target, e)))
    }
}
impl RaftNetworkFactory<Types> for Network {
    type Network = Peer;
    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Peer {
        Peer {
            network: self.clone(),
            target,
            node: node.clone(),
        }
    }
}
impl RaftNetwork<Types> for Peer {
    async fn append_entries(
        &mut self,
        req: AppendEntriesRequest<Types>,
        _: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RpcErr> {
        self.network
            .rpc(self.target, &self.node, "append", req)
            .await
    }
    async fn vote(
        &mut self,
        req: VoteRequest<u64>,
        _: RPCOption,
    ) -> Result<VoteResponse<u64>, RpcErr> {
        self.network.rpc(self.target, &self.node, "vote", req).await
    }
    async fn install_snapshot(
        &mut self,
        req: InstallSnapshotRequest<Types>,
        _: RPCOption,
    ) -> Result<InstallSnapshotResponse<u64>, RpcErr<InstallSnapshotError>> {
        self.network
            .rpc(self.target, &self.node, "snapshot", req)
            .await
    }
}

pub struct Cluster {
    pub raft: Raft,
    pub config: ClusterConfig,
    pub machine: Arc<Machine>,
    network: Network,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ReadRequest {
    pub volume: String,
    pub path: String,
}
impl Cluster {
    pub async fn start(
        engine: MeshFSEngine,
        config: ClusterConfig,
        bootstrap: bool,
    ) -> io::Result<Arc<Self>> {
        let token = std::env::var("MESHFS_CLUSTER_TOKEN")
            .map_err(|_| err("MESHFS_CLUSTER_TOKEN is required"))?;
        if token.len() < 32 {
            return Err(err("cluster token must contain at least 32 bytes"));
        }
        let fingerprint = config.fingerprint();
        let dir = engine.data_dir.join("raft");
        std::fs::create_dir_all(&dir)?;
        let db = Db::open(&dir.join("state.sqlite"))?;
        if let Some(previous) = db.get::<String>("topology").map_err(err)? {
            if previous != fingerprint {
                return Err(err(
                    "changing configured membership requires a separate migration",
                ));
            }
        } else {
            let s = engine.state.read().await;
            if s.volumes.values().any(|v| !v.files.is_empty()) || !s.snapshots.is_empty() {
                return Err(err("cluster bootstrap requires a dedicated empty namespace; existing local data must be explicitly imported"));
            }
            db.set("topology", &fingerprint).map_err(err)?;
            db.set("node_id", &config.node_id).map_err(err)?;
        }
        if db.get::<u64>("node_id").map_err(err)? != Some(config.node_id) {
            return Err(err("persisted node identity differs from configuration"));
        }
        {
            let mut state = engine.state.write().await;
            state.node_id = config.node_id.to_string();
            state.cluster_name = config.cluster.clone();
        }
        let data = db
            .get::<MachineData>("machine")
            .map_err(err)?
            .unwrap_or_default();
        let mut local_engine = engine.clone();
        local_engine.cluster = Arc::new(std::sync::OnceLock::new());
        let machine = Arc::new(Machine {
            db: db.clone(),
            engine: local_engine,
            data: AsyncMutex::new(data),
        });
        {
            let s = machine.data.lock().await;
            machine.mirror(&s).await;
        }
        let network = Network {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(4))
                .build()
                .map_err(err)?,
            token: Arc::from(token),
            fingerprint,
        };
        let raft_config = openraft::Config {
            cluster_name: config.cluster.clone(),
            heartbeat_interval: 250,
            max_payload_entries: 1,
            install_snapshot_timeout: 120_000,
            snapshot_max_chunk_size: 256 * 1024,
            election_timeout_min: 800,
            election_timeout_max: 1600,
            snapshot_policy: openraft::SnapshotPolicy::LogsSinceLast(64),
            max_in_snapshot_log_to_keep: 32,
            ..Default::default()
        }
        .validate()
        .map_err(err)?;
        let raft = Raft::new(
            config.node_id,
            Arc::new(raft_config),
            network.clone(),
            db,
            machine.clone(),
        )
        .await
        .map_err(err)?;
        let out = Arc::new(Self {
            raft,
            config,
            machine,
            network,
        });
        // The topology is fixed and identical on all nodes; no automatic shrinking.
        if bootstrap {
            if out.raft.is_initialized().await.map_err(err)? {
                return Err(err("bootstrap refused: this node is already initialized"));
            }
            out.raft.initialize(out.config.nodes()).await.map_err(err)?;
        }
        Ok(out)
    }
    pub fn authorized(&self, headers: &axum::http::HeaderMap) -> bool {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let expected = format!("Bearer {}", self.network.token);
        let equal = provided.len() == expected.len()
            && provided
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |a, (x, y)| a | (x ^ y))
                == 0;
        equal
            && headers
                .get("x-meshfs-topology")
                .and_then(|v| v.to_str().ok())
                == Some(self.network.fingerprint.as_str())
    }
    fn leader(&self) -> io::Result<(u64, BasicNode)> {
        let id = self
            .raft
            .metrics()
            .borrow()
            .current_leader
            .ok_or_else(|| err("leader election in progress"))?;
        let addr = self
            .config
            .peers
            .get(&id)
            .ok_or_else(|| err("unknown leader"))?;
        Ok((id, BasicNode::new(addr.clone())))
    }
    pub async fn local_write(&self, c: Command) -> Result<Value, String> {
        if c.id.is_empty() || c.id.len() > 256 {
            return Err("invalid operation ID".into());
        }
        if let Action::StageChunk {hex} = &c.action {
            if hex.len()>CHUNK_SIZE*2 {return Err("oversized chunk payload".into());}
            let bytes=hex::decode(hex).map_err(|e|e.to_string())?;
            if bytes.is_empty(){return Err("empty staged chunk".into());}
            let id=format!("sha256:{}",sha256_digest(&bytes));
            let already_committed=self.machine.data.lock().await.blobs.contains_key(&id);
            if already_committed {
                tokio::time::timeout(std::time::Duration::from_secs(3),self.raft.ensure_linearizable()).await
                    .map_err(|_|"quorum deadline exceeded".to_string())?.map_err(|e|e.to_string())?;
                return Ok(json!(id));
            }
        }
        if let Action::Put { hex, .. } = &c.action {
            if hex.len() > MAX_INLINE_BYTES * 2 || hex::decode(hex).is_err() {
                return Err("invalid or oversized file payload".into());
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), self.raft.client_write(c))
            .await
            .map_err(|_| {
                "write deadline exceeded; outcome may be committed, reconcile before retry"
                    .to_string()
            })?
            .map(|r| r.data)
            .map_err(|e| e.to_string())
    }
    pub async fn write(&self, action: Action) -> io::Result<Value> {
        let id = format!(
            "{}-{}",
            self.config.node_id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(err)?
                .as_nanos()
        );
        let command = Command { id, action };
        let (leader, node) = self.leader()?;
        let value = if leader == self.config.node_id {
            self.local_write(command).await.map_err(err)?
        } else {
            self.network
                .rpc::<_, Value, StringError>(leader, &node, "write", command)
                .await
                .map_err(err)?
        };
        if let Some(e) = value.get("error") {
            return Err(err(e));
        }
        Ok(value)
    }
    pub async fn local_read(&self, r: ReadRequest) -> Result<String, ReadError> {
        tokio::time::timeout(std::time::Duration::from_secs(3), self.raft.ensure_linearizable())
            .await.map_err(|_| ReadError::unavailable("quorum read deadline exceeded"))?
            .map_err(ReadError::unavailable)?;
        if self.machine.engine.local_inode(&r.volume,&r.path).await.map_err(ReadError::unavailable)?.size>MAX_INLINE_BYTES as u64 {
            return Err(ReadError::unavailable("use the streaming download endpoint for large files"));
        }
        self.machine.engine.local_get_file(&r.volume, &r.path).await.map(hex::encode)
            .map_err(|e| ReadError { missing: e.kind()==io::ErrorKind::NotFound, message: e.to_string() })
    }
    pub async fn read(&self, volume: &str, path: &str) -> io::Result<Vec<u8>> {
        let (leader, node) = self.leader()?;
        let req = ReadRequest { volume: volume.into(), path: path.into() };
        let data = if leader == self.config.node_id {
            self.local_read(req).await.map_err(ReadError::into_io)?
        } else {
            self.network.rpc::<_, String, ReadError>(leader, &node, "read", req).await
                .map_err(|e| match e {
                    RPCError::RemoteError(remote) => remote.source.into_io(),
                    other => err(other),
                })?
        };
        hex::decode(data).map_err(err)
    }

    pub async fn local_inode(&self,r:ReadRequest)->Result<Inode,ReadError> {
        tokio::time::timeout(std::time::Duration::from_secs(3),self.raft.ensure_linearizable()).await
            .map_err(|_|ReadError::unavailable("quorum read deadline exceeded"))?.map_err(ReadError::unavailable)?;
        self.machine.engine.local_inode(&r.volume,&r.path).await.map_err(|e|ReadError{missing:e.kind()==io::ErrorKind::NotFound,message:e.to_string()})
    }
    pub async fn inode(&self,volume:&str,path:&str)->io::Result<Inode> {
        let (leader,node)=self.leader()?;let req=ReadRequest{volume:volume.into(),path:path.into()};
        if leader==self.config.node_id {self.local_inode(req).await.map_err(ReadError::into_io)} else {
            self.network.rpc::<_,Inode,ReadError>(leader,&node,"inode",req).await.map_err(|e|match e{RPCError::RemoteError(r)=>r.source.into_io(),other=>err(other)})
        }
    }
    pub fn local_chunk(&self,id:&str)->Result<String,String> {self.machine.engine.chunk_store.read_chunk(id).map(hex::encode).map_err(|e|e.to_string())}
    pub async fn chunk(&self,id:&str)->io::Result<Vec<u8>> {
        if let Ok(data)=self.machine.engine.chunk_store.read_chunk(id){return Ok(data);}
        for (node,url) in &self.config.peers {
            if *node==self.config.node_id{continue;}
            if let Ok(data)=self.network.rpc::<_,String,StringError>(*node,&BasicNode::new(url.clone()),"chunk",id).await {
                let bytes=hex::decode(data).map_err(err)?;
                if self.machine.engine.chunk_store.write_chunk(&bytes)?!=id {return Err(err("peer chunk hash mismatch"));}
                return Ok(bytes);
            }
        }
        Err(err("referenced chunk is unavailable from configured replicas"))
    }
    pub async fn local_namespace(&self) -> Result<Value, String> {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.raft.ensure_linearizable(),
        )
        .await
        .map_err(|_| "quorum read deadline exceeded".to_string())?
        .map_err(|e| e.to_string())?;
        let s = self.machine.data.lock().await;
        Ok(json!({"volumes":s.volumes,"snapshots":s.snapshots}))
    }
    pub async fn namespace(&self) -> io::Result<Value> {
        let (leader, node) = self.leader()?;
        if leader == self.config.node_id {
            self.local_namespace().await.map_err(err)
        } else {
            self.network
                .rpc::<_, Value, StringError>(leader, &node, "namespace", ())
                .await
                .map_err(err)
        }
    }
    pub async fn nodes(&self) -> Vec<crate::engine::NodeInfo> {
        let mut nodes = Vec::new();
        for (id, url) in &self.config.peers {
            let reachable = if *id == self.config.node_id {
                self.raft.metrics().borrow().running_state.is_ok()
            } else {
                self.network
                    .rpc::<_, Value, StringError>(*id, &BasicNode::new(url.clone()), "health", ())
                    .await
                    .is_ok()
            };
            nodes.push(crate::engine::NodeInfo {
                node_id: id.to_string(),
                hostname: url.clone(),
                address: url.clone(),
                role: if self.raft.metrics().borrow().current_leader == Some(*id) {
                    "leader"
                } else {
                    "replica"
                }
                .into(),
                status: if reachable { "reachable" } else { "offline" }.into(),
                joined_at: String::new(),
            });
        }
        nodes
    }
    pub fn metrics(&self) -> Value {
        serde_json::to_value(&*self.raft.metrics().borrow()).unwrap_or(Value::Null)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringError(pub String);
impl std::fmt::Display for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StringError {}

#[cfg(test)]
mod tests {
    use super::*;
    use openraft::testing::StoreBuilder;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct Builder;
    impl openraft::testing::StoreBuilder<Types, Db, Arc<Machine>, Cleanup> for Builder {
        async fn build(&self) -> SResult<(Cleanup, Db, Arc<Machine>)> {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "meshfs-raft-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).map_err(se)?;
            let db = Db::open(&dir.join("raft.sqlite")).map_err(se)?;
            let engine = MeshFSEngine::new(&dir, "test", 0).await.map_err(se)?;
            let sm = Arc::new(Machine {
                db: db.clone(),
                engine,
                data: AsyncMutex::new(MachineData::default()),
            });
            Ok((Cleanup(dir), db, sm))
        }
    }
    #[test]
    fn raft_storage_contract() {
        openraft::testing::Suite::test_all(Builder).unwrap();
    }
    #[tokio::test]
    async fn committed_files_survive_reopen_and_snapshot_transfer() {
        let (_guard, db, mut sm) = Builder.build().await.unwrap();
        let command = Command {
            id: "write-1".into(),
            action: Action::Put {
                volume: "default".into(),
                path: "test.txt".into(),
                hex: hex::encode(b"durable replicated bytes"),
                at: "2026-01-01T00:00:00Z".into(),
            },
        };
        let entry = Entry {
            log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), 1),
            payload: EntryPayload::Normal(command.clone()),
        };
        sm.apply([entry]).await.unwrap();
        let persisted = db.get::<MachineData>("machine").unwrap().unwrap();
        let reopened = Db::open(&_guard.0.join("raft.sqlite")).unwrap();
        assert_eq!(
            reopened
                .get::<MachineData>("machine")
                .unwrap()
                .unwrap()
                .applied,
            persisted.applied
        );
        let (_target_guard, _, mut target) = Builder.build().await.unwrap();
        let image = sm.build_snapshot().await.unwrap();
        target
            .install_snapshot(&image.meta, image.snapshot)
            .await
            .unwrap();
        assert_eq!(
            target
                .engine
                .local_get_file("default", "test.txt")
                .await
                .unwrap(),
            b"durable replicated bytes"
        );
        let mut state = target.data.lock().await;
        let before = state.volumes["default"].merkle_root.clone();
        target.execute(&mut state, &command).unwrap();
        assert_eq!(before, state.volumes["default"].merkle_root);
        let reused = Command {
            id: command.id,
            action: Action::DeleteVolume {
                name: "default".into(),
            },
        };
        assert!(target
            .execute(&mut state, &reused)
            .unwrap()
            .get("error")
            .is_some());
        assert!(state.volumes.contains_key("default"));
    }
}

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ReadError { pub missing:bool, pub message:String }
impl ReadError {
    fn unavailable(e:impl std::fmt::Display)->Self {Self{missing:false,message:e.to_string()}}
    fn into_io(self)->io::Error {io::Error::new(if self.missing {io::ErrorKind::NotFound}else{io::ErrorKind::Other},self.message)}
}
impl std::fmt::Display for ReadError {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str(&self.message)}}
impl std::error::Error for ReadError {}

fn write_frame(w:&mut impl Write,bytes:&[u8])->SResult<()> {
    w.write_all(&(bytes.len() as u64).to_be_bytes()).map_err(se)?;w.write_all(bytes).map_err(se)
}
fn read_frame(r:&mut impl Read,max:usize)->SResult<Vec<u8>> {
    let mut len=[0;8];r.read_exact(&mut len).map_err(se)?;let len=u64::from_be_bytes(len);
    if len>max as u64{return Err(se("snapshot frame exceeds its bound"));}
    let mut bytes=vec![0;len as usize];r.read_exact(&mut bytes).map_err(se)?;Ok(bytes)
}

/// Payload hashes are calculated while ingesting and verified while reading.
/// Committing an inode checks references to already committed immutable blocks;
/// it never rescans an entire VM disk while holding the state-machine lock.
fn validate_layout(inode:&Inode,sizes:&BTreeMap<String,usize>)->SResult<()> {
    if inode.chunks_count!=inode.chunks.len(){return Err(se("chunk count mismatch"));}
    let digest=inode.digest.strip_prefix("sha256:").ok_or_else(||se("invalid file digest"))?;
    if digest.len()!=64 || !digest.bytes().all(|b|b.is_ascii_hexdigit()){return Err(se("invalid file digest"));}
    let mut offset=0u64;
    for c in &inode.chunks {
        if c.offset!=offset || c.size==0 || c.size>CHUNK_SIZE || sizes.get(&c.id)!=Some(&c.size){return Err(se("uncommitted or invalid chunk reference"));}
        offset=offset.checked_add(c.size as u64).ok_or_else(||se("file size overflow"))?;
    }
    if offset!=inode.size{return Err(se("file size mismatch"));}Ok(())
}
