//! Quorum-committed MeshFS namespaces. Raft orders mutations; SQLite durably stores
//! votes, logs and applied state. A returning peer catches up through log replay
//! or a snapshot containing both metadata and immutable CAS content.
use std::{collections::{BTreeMap, HashMap}, fmt::Debug, io::{self, Cursor}, path::Path, ops::RangeBounds, sync::{Arc, Mutex}};
use openraft::{BasicNode, Entry, EntryPayload, LogId, LogState, RaftLogReader, RaftSnapshotBuilder, RaftTypeConfig, Snapshot, SnapshotMeta, StoredMembership, Vote, StorageError, StorageIOError};
use openraft::storage::{RaftLogStorage, RaftStateMachine, LogFlushed};
use openraft::network::{RaftNetworkFactory, RaftNetwork, RPCOption};
use openraft::raft::*;
use openraft::error::{RPCError, RaftError, NetworkError, RemoteError, InstallSnapshotError};
use rusqlite::{Connection, params, OptionalExtension};
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex};
use crate::{engine::MeshFSEngine, chunk::{ChunkRef, CHUNK_SIZE, sha256_digest}, merkle::Inode, volume::{Volume, Snapshot as FileSnapshot}};

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Put { volume: String, path: String, hex: String, at: String },
    CreateVolume { name: String, at: String },
    DeleteVolume { name: String },
    Snapshot { volume: String, name: String, id: String, at: String },
    DeleteSnapshot { volume: String, name: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command { pub id: String, pub action: Action }
openraft::declare_raft_types!(pub Types: D = Command, R = Value);
pub type Raft = openraft::Raft<Types>;
type SResult<T> = Result<T, StorageError<u64>>;
type RpcErr<E = openraft::error::Infallible> = RPCError<u64, BasicNode, RaftError<u64, E>>;
fn err(e: impl std::fmt::Display) -> io::Error { io::Error::other(e.to_string()) }
fn se(e: impl std::fmt::Display) -> StorageError<u64> { StorageIOError::write_state_machine(&err(e)).into() }

#[derive(Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    pub cluster: String,
    pub node_id: u64,
    pub peers: BTreeMap<u64, String>,
}
impl ClusterConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        let c: Self = serde_json::from_slice(&std::fs::read(path)?).map_err(err)?;
        if c.cluster.is_empty() || !c.peers.contains_key(&c.node_id) || c.peers.len()<3 || c.peers.len()%2==0 { return Err(err("cluster requires an odd fixed membership of at least three nodes, including this node")); }
        let mut urls = std::collections::HashSet::new();
        for url in c.peers.values() {
            let u = reqwest::Url::parse(url).map_err(err)?;
            if !matches!(u.scheme(), "http"|"https") || u.host_str().is_none() || u.path()!="/" || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() || u.password().is_some() || !urls.insert(url.trim_end_matches('/').to_string()) { return Err(err("invalid or duplicate peer URL")); }
        }
        Ok(c)
    }
    fn fingerprint(&self) -> String { sha256_digest(&serde_json::to_vec(&(&self.cluster,&self.peers)).expect("serializable topology")) }
    fn nodes(&self) -> BTreeMap<u64,BasicNode> { self.peers.iter().map(|(id,url)|(*id,BasicNode::new(url.clone()))).collect() }
}

#[derive(Clone)]
pub struct Db { conn: Arc<Mutex<Connection>> }
impl Db {
    fn open(path: &Path) -> io::Result<Self> {
        let c=Connection::open(path).map_err(err)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY,v TEXT NOT NULL); CREATE TABLE IF NOT EXISTS logs(idx INTEGER PRIMARY KEY,entry TEXT NOT NULL);").map_err(err)?;
        Ok(Self{conn:Arc::new(Mutex::new(c))})
    }
    fn get<T: DeserializeOwned>(&self,k:&str)->SResult<Option<T>> {
        let c=self.conn.lock().map_err(se)?;
        let s:Option<String>=c.query_row("SELECT v FROM meta WHERE k=?",[k],|r|r.get(0)).optional().map_err(se)?;
        s.map(|s|serde_json::from_str(&s).map_err(se)).transpose()
    }
    fn set<T: Serialize>(&self,k:&str,v:&T)->SResult<()> {
        self.conn.lock().map_err(se)?.execute("INSERT INTO meta(k,v) VALUES(?,?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",params![k,serde_json::to_string(v).map_err(se)?]).map_err(se)?;Ok(())
    }
}
impl RaftLogReader<Types> for Db {
    async fn try_get_log_entries<R:RangeBounds<u64>+Clone+Debug+Send>(&mut self,range:R)->SResult<Vec<Entry<Types>>> {
        let c=self.conn.lock().map_err(se)?;
        let mut q=c.prepare("SELECT idx,entry FROM logs ORDER BY idx").map_err(se)?;
        let rows=q.query_map([],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?))).map_err(se)?;
        let mut out=Vec::new();for row in rows { let (i,s)=row.map_err(se)?;if range.contains(&i){out.push(serde_json::from_str(&s).map_err(se)?);} }Ok(out)
    }
    async fn read_vote(&mut self)->SResult<Option<Vote<u64>>> {self.get("vote")}
}
impl RaftLogStorage<Types> for Db {
    type LogReader=Self;
    async fn get_log_state(&mut self)->SResult<LogState<Types>> {
        let purged:Option<LogId<u64>>=self.get("purged")?;
        let c=self.conn.lock().map_err(se)?;
        let row:Option<String>=c.query_row("SELECT entry FROM logs ORDER BY idx DESC LIMIT 1",[],|r|r.get(0)).optional().map_err(se)?;
        let last=match row {Some(s)=>Some(serde_json::from_str::<Entry<Types>>(&s).map_err(se)?.log_id),None=>purged};
        Ok(LogState{last_purged_log_id:purged,last_log_id:last})
    }
    async fn get_log_reader(&mut self)->Self {self.clone()}
    async fn save_vote(&mut self,v:&Vote<u64>)->SResult<()> {self.set("vote",v)}
    async fn read_vote(&mut self)->SResult<Option<Vote<u64>>> {self.get("vote")}
    async fn save_committed(&mut self,v:Option<LogId<u64>>)->SResult<()> {self.set("committed",&v)}
    async fn read_committed(&mut self)->SResult<Option<LogId<u64>>> {Ok(self.get::<Option<LogId<u64>>>("committed")?.flatten())}
    async fn append<I>(&mut self,entries:I,callback:LogFlushed<Types>)->SResult<()> where I:IntoIterator<Item=Entry<Types>>+Send,I::IntoIter:Send {
        {let mut c=self.conn.lock().map_err(se)?;let tx=c.transaction().map_err(se)?;
        for e in entries {tx.execute("INSERT INTO logs(idx,entry) VALUES(?,?) ON CONFLICT(idx) DO UPDATE SET entry=excluded.entry",params![e.log_id.index,serde_json::to_string(&e).map_err(se)?]).map_err(se)?;}
        tx.commit().map_err(se)?;}
        callback.log_io_completed(Ok(()));Ok(())
    }
    async fn truncate(&mut self,id:LogId<u64>)->SResult<()> {self.conn.lock().map_err(se)?.execute("DELETE FROM logs WHERE idx>=?",[id.index]).map_err(se)?;Ok(())}
    async fn purge(&mut self,id:LogId<u64>)->SResult<()> {
        let mut c=self.conn.lock().map_err(se)?;let tx=c.transaction().map_err(se)?;
        tx.execute("DELETE FROM logs WHERE idx<=?",[id.index]).map_err(se)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('purged',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",[serde_json::to_string(&id).map_err(se)?]).map_err(se)?;
        tx.commit().map_err(se)?;Ok(())
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct MachineData {
    applied:Option<LogId<u64>>,membership:StoredMembership<u64,BasicNode>,
    volumes:HashMap<String,Volume>,snapshots:Vec<FileSnapshot>,
    seen:BTreeMap<String,(String,Value)>,
}
#[derive(Clone,Serialize,Deserialize)]
struct Image { state:MachineData,chunks:BTreeMap<String,String> }
pub struct Machine { db:Db, engine:MeshFSEngine,data:AsyncMutex<MachineData> }
impl Machine {
    async fn mirror(&self,data:&MachineData) {
        let mut s=self.engine.state.write().await;s.volumes=data.volumes.clone();s.snapshots=data.snapshots.clone();
    }
    fn execute(&self,s:&mut MachineData,c:&Command)->SResult<Value> {
        let digest=sha256_digest(&serde_json::to_vec(&c.action).map_err(se)?);
        if let Some((previous,result))=s.seen.get(&c.id) {return Ok(if previous==&digest {result.clone()} else {json!({"error":"operation ID reused for different content"})});}
        let result=match &c.action {
            Action::Put{volume,path,hex,at}=>{
                let bytes=hex::decode(hex).map_err(se)?;
                let mut chunks=Vec::new();let mut offset=0;
                for part in bytes.chunks(CHUNK_SIZE) {let id=self.engine.chunk_store.write_chunk(part).map_err(se)?;chunks.push(ChunkRef{id,size:part.len(),offset});offset+=part.len() as u64;}
                let inode=Inode{path:path.clone(),size:bytes.len() as u64,digest:format!("sha256:{}",sha256_digest(&bytes)),chunks_count:chunks.len(),chunks,updated_at:at.clone()};
                let vol=s.volumes.entry(volume.clone()).or_insert_with(||{let mut v=Volume::new(volume);v.created_at=at.clone();v});
                vol.insert_file(inode.clone());json!(inode)
            },
            Action::CreateVolume{name,at}=>{let v=s.volumes.entry(name.clone()).or_insert_with(||{let mut v=Volume::new(name);v.created_at=at.clone();v});json!(v)},
            Action::DeleteVolume{name}=>json!(s.volumes.remove(name).is_some()),
            Action::Snapshot{volume,name,id,at}=>{
                match s.volumes.get(volume) {None=>json!({"error":"volume not found"}),Some(v)=>{
                    let snap=FileSnapshot{id:id.clone(),name:name.clone(),volume:volume.clone(),merkle_root:v.merkle_root.clone(),files_count:v.files.len(),total_bytes:v.total_bytes(),created_at:at.clone(),files_tree:v.files.clone()};
                    s.snapshots.insert(0,snap.clone());json!(snap)
                }}
            },
            Action::DeleteSnapshot{volume,name}=>{let len=s.snapshots.len();s.snapshots.retain(|v|!(v.volume==*volume&&(v.name==*name||v.id==*name)));json!(len!=s.snapshots.len())},
        };
        s.seen.insert(c.id.clone(),(digest,result.clone()));Ok(result)
    }
    fn image(&self,s:&MachineData)->SResult<Image> {
        let mut chunks=BTreeMap::new();
        for inode in s.volumes.values().flat_map(|v|v.files.values()).chain(s.snapshots.iter().flat_map(|v|v.files_tree.values())) {
            for c in &inode.chunks {if !chunks.contains_key(&c.id){chunks.insert(c.id.clone(),hex::encode(self.engine.chunk_store.read_chunk(&c.id).map_err(se)?));}}
        }
        Ok(Image{state:s.clone(),chunks})
    }
}
impl RaftStateMachine<Types> for Arc<Machine> {
    type SnapshotBuilder=Self;
    async fn applied_state(&mut self)->SResult<(Option<LogId<u64>>,StoredMembership<u64,BasicNode>)> {let s=self.data.lock().await;Ok((s.applied,s.membership.clone()))}
    async fn apply<I>(&mut self,entries:I)->SResult<Vec<Value>> where I:IntoIterator<Item=Entry<Types>>+Send,I::IntoIter:Send {
        let entries:Vec<_>=entries.into_iter().collect();let mut s=self.data.lock().await;let mut out=Vec::new();
        for e in entries {
            let value=match e.payload {EntryPayload::Blank=>Value::Null,EntryPayload::Membership(m)=>{s.membership=StoredMembership::new(Some(e.log_id),m);Value::Null},EntryPayload::Normal(c)=>self.execute(&mut s,&c)?};
            s.applied=Some(e.log_id);out.push(value);
        }
        self.db.set("machine",&*s)?;self.mirror(&s).await;Ok(out)
    }
    async fn get_snapshot_builder(&mut self)->Self {self.clone()}
    async fn begin_receiving_snapshot(&mut self)->SResult<Box<<Types as RaftTypeConfig>::SnapshotData>> {Ok(Box::new(Cursor::new(Vec::new())))}
    async fn install_snapshot(&mut self,meta:&SnapshotMeta<u64,BasicNode>,snapshot:Box<<Types as RaftTypeConfig>::SnapshotData>)->SResult<()> {
        let image:Image=serde_json::from_slice(snapshot.get_ref()).map_err(se)?;
        if image.state.applied!=meta.last_log_id||image.state.membership!=meta.last_membership{return Err(se("snapshot metadata mismatch"));}
        // Only immutable chunks are installed before the namespace transaction.
        for (id,value) in &image.chunks {let bytes=hex::decode(value).map_err(se)?;if self.engine.chunk_store.write_chunk(&bytes).map_err(se)?!=*id{return Err(se("snapshot chunk hash mismatch"));}}
        for inode in image.state.volumes.values().flat_map(|v|v.files.values()).chain(image.state.snapshots.iter().flat_map(|s|s.files_tree.values())) {for c in &inode.chunks {self.engine.chunk_store.read_chunk(&c.id).map_err(se)?;}}
        let mut s=self.data.lock().await;self.db.set("machine",&image.state)?;*s=image.state;self.mirror(&s).await;Ok(())
    }
    async fn get_current_snapshot(&mut self)->SResult<Option<Snapshot<Types>>> {
        if self.data.lock().await.applied.is_none(){return Ok(None)}
        self.build_snapshot().await.map(Some)
    }
}
impl RaftSnapshotBuilder<Types> for Arc<Machine> {
    async fn build_snapshot(&mut self)->SResult<Snapshot<Types>> {
        let s=self.data.lock().await;let image=self.image(&s)?;
        let meta=SnapshotMeta{last_log_id:s.applied,last_membership:s.membership.clone(),snapshot_id:format!("{:?}",s.applied)};
        Ok(Snapshot{meta,snapshot:Box::new(Cursor::new(serde_json::to_vec(&image).map_err(se)?))})
    }
}

#[derive(Clone)]
pub struct Network { client:reqwest::Client,token:Arc<str>,fingerprint:String }
pub struct Peer { network:Network,target:u64,node:BasicNode }
impl Network {
    async fn rpc<Q:Serialize,R:DeserializeOwned,E:std::error::Error+DeserializeOwned>(&self,target:u64,node:&BasicNode,path:&str,req:Q)->Result<R,RPCError<u64,BasicNode,E>> {
        let response=self.client.post(format!("{}/internal/{}",node.addr.trim_end_matches('/'),path)).bearer_auth(&*self.token).header("x-meshfs-topology",&self.fingerprint).json(&req).send().await.map_err(|e|RPCError::Network(NetworkError::new(&e)))?;
        let result:Result<R,E>=response.error_for_status().map_err(|e|RPCError::Network(NetworkError::new(&e)))?.json().await.map_err(|e|RPCError::Network(NetworkError::new(&e)))?;
        result.map_err(|e|RPCError::RemoteError(RemoteError::new(target,e)))
    }
}
impl RaftNetworkFactory<Types> for Network {
    type Network=Peer;
    async fn new_client(&mut self,target:u64,node:&BasicNode)->Peer {Peer{network:self.clone(),target,node:node.clone()}}
}
impl RaftNetwork<Types> for Peer {
    async fn append_entries(&mut self,req:AppendEntriesRequest<Types>,_:RPCOption)->Result<AppendEntriesResponse<u64>,RpcErr> {self.network.rpc(self.target,&self.node,"append",req).await}
    async fn vote(&mut self,req:VoteRequest<u64>,_:RPCOption)->Result<VoteResponse<u64>,RpcErr> {self.network.rpc(self.target,&self.node,"vote",req).await}
    async fn install_snapshot(&mut self,req:InstallSnapshotRequest<Types>,_:RPCOption)->Result<InstallSnapshotResponse<u64>,RpcErr<InstallSnapshotError>> {self.network.rpc(self.target,&self.node,"snapshot",req).await}
}

pub struct Cluster {
    pub raft:Raft,pub config:ClusterConfig,pub machine:Arc<Machine>,network:Network,
}
#[derive(Clone,Serialize,Deserialize)]
pub struct ReadRequest {pub volume:String,pub path:String}
impl Cluster {
    pub async fn start(engine:MeshFSEngine,config:ClusterConfig,bootstrap:bool)->io::Result<Arc<Self>> {
        let token=std::env::var("MESHFS_CLUSTER_TOKEN").map_err(|_|err("MESHFS_CLUSTER_TOKEN is required"))?;
        if token.len()<32{return Err(err("cluster token must contain at least 32 bytes"))}
        let fingerprint=config.fingerprint();let dir=engine.data_dir.join("raft");std::fs::create_dir_all(&dir)?;
        let db=Db::open(&dir.join("state.sqlite"))?;
        if let Some(previous)=db.get::<String>("topology").map_err(err)? {if previous!=fingerprint {return Err(err("changing configured membership requires a separate migration"))}}
        else {
            let s=engine.state.read().await;
            if s.volumes.values().any(|v|!v.files.is_empty())||!s.snapshots.is_empty(){return Err(err("cluster bootstrap requires a dedicated empty namespace; existing local data must be explicitly imported"))}
            db.set("topology",&fingerprint).map_err(err)?;db.set("node_id",&config.node_id).map_err(err)?;
        }
        if db.get::<u64>("node_id").map_err(err)?!=Some(config.node_id){return Err(err("persisted node identity differs from configuration"))}
        let data=db.get::<MachineData>("machine").map_err(err)?.unwrap_or_default();
        let mut local_engine = engine.clone();
        local_engine.cluster = Arc::new(std::sync::OnceLock::new());
        let machine=Arc::new(Machine{db:db.clone(),engine:local_engine,data:AsyncMutex::new(data)});
        {let s=machine.data.lock().await;machine.mirror(&s).await;}
        let network=Network{client:reqwest::Client::builder().timeout(std::time::Duration::from_secs(4)).build().map_err(err)?,token:Arc::from(token),fingerprint};
        let raft_config=openraft::Config{cluster_name:config.cluster.clone(),heartbeat_interval:250,election_timeout_min:800,election_timeout_max:1600,snapshot_policy:openraft::SnapshotPolicy::LogsSinceLast(64),max_in_snapshot_log_to_keep:32,..Default::default()}.validate().map_err(err)?;
        let raft=Raft::new(config.node_id,Arc::new(raft_config),network.clone(),db,machine.clone()).await.map_err(err)?;
        let out=Arc::new(Self{raft,config,machine,network});
        // The topology is fixed and identical on all nodes; no automatic shrinking.
        if bootstrap {
            if out.raft.is_initialized().await.map_err(err)? {return Err(err("bootstrap refused: this node is already initialized"));}
            out.raft.initialize(out.config.nodes()).await.map_err(err)?;
        }
        Ok(out)
    }
    pub fn authorized(&self,headers:&axum::http::HeaderMap)->bool {
        let provided=headers.get("authorization").and_then(|v|v.to_str().ok()).unwrap_or("");
        let expected=format!("Bearer {}",self.network.token);
        let equal=provided.len()==expected.len()&&provided.bytes().zip(expected.bytes()).fold(0u8,|a,(x,y)|a|(x^y))==0;
        equal&&headers.get("x-meshfs-topology").and_then(|v|v.to_str().ok())==Some(self.network.fingerprint.as_str())
    }
    fn leader(&self)->io::Result<(u64,BasicNode)> {let id=self.raft.metrics().borrow().current_leader.ok_or_else(||err("leader election in progress"))?;let addr=self.config.peers.get(&id).ok_or_else(||err("unknown leader"))?;Ok((id,BasicNode::new(addr.clone())))}
    pub async fn local_write(&self,c:Command)->Result<Value,String> {
        if c.id.is_empty() || c.id.len()>256 { return Err("invalid operation ID".into()); }
        if let Action::Put { hex, .. } = &c.action {
            if hex.len()>MAX_FILE_BYTES*2 || hex::decode(hex).is_err() { return Err("invalid or oversized file payload".into()); }
        }
        self.raft.client_write(c).await.map(|r|r.data).map_err(|e|e.to_string())
    }
    pub async fn write(&self,action:Action)->io::Result<Value> {
        let id=format!("{}-{}",self.config.node_id,std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(err)?.as_nanos());
        let command=Command{id,action};let (leader,node)=self.leader()?;
        let value=if leader==self.config.node_id{self.local_write(command).await.map_err(err)?}else{self.network.rpc::<_,Value,StringError>(leader,&node,"write",command).await.map_err(err)?};
        if let Some(e)=value.get("error"){return Err(err(e))}Ok(value)
    }
    pub async fn local_read(&self,r:ReadRequest)->Result<String,String> {
        self.raft.ensure_linearizable().await.map_err(|e|e.to_string())?;
        self.machine.engine.local_get_file(&r.volume,&r.path).await.map(hex::encode).map_err(|e|e.to_string())
    }
    pub async fn read(&self,volume:&str,path:&str)->io::Result<Vec<u8>> {
        let (leader,node)=self.leader()?;let req=ReadRequest{volume:volume.to_string(),path:path.to_string()};
        let data=if leader==self.config.node_id{self.local_read(req).await.map_err(err)?}else{self.network.rpc::<_,String,StringError>(leader,&node,"read",req).await.map_err(err)?};hex::decode(data).map_err(err)
    }
    pub async fn local_namespace(&self)->Result<Value,String> {
        self.raft.ensure_linearizable().await.map_err(|e|e.to_string())?;
        let s=self.machine.data.lock().await;
        Ok(json!({"volumes":s.volumes,"snapshots":s.snapshots}))
    }
    pub async fn namespace(&self)->io::Result<Value> {
        let (leader,node)=self.leader()?;
        if leader==self.config.node_id {self.local_namespace().await.map_err(err)}
        else {self.network.rpc::<_,Value,StringError>(leader,&node,"namespace",()).await.map_err(err)}
    }
    pub async fn nodes(&self)->Vec<crate::engine::NodeInfo> {
        let mut nodes=Vec::new();
        for (id,url) in &self.config.peers {
            let reachable = if *id==self.config.node_id {self.raft.metrics().borrow().running_state.is_ok()} else {
                self.network.rpc::<_,Value,StringError>(*id,&BasicNode::new(url.clone()),"health",()).await.is_ok()
            };
            nodes.push(crate::engine::NodeInfo { node_id:id.to_string(), hostname: url.clone(), address:url.clone(),
                role:if self.raft.metrics().borrow().current_leader==Some(*id){"leader"}else{"replica"}.into(),
                status:if reachable{"reachable"}else{"offline"}.into(), joined_at:String::new() });
        }
        nodes
    }
    pub fn metrics(&self)->Value {serde_json::to_value(&*self.raft.metrics().borrow()).unwrap_or(Value::Null)}
}
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct StringError(pub String);
impl std::fmt::Display for StringError {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str(&self.0)}}
impl std::error::Error for StringError {}
