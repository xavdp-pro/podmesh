//! Quorum-committed MeshFS namespaces. Raft orders mutations; SQLite durably stores
//! votes, logs and applied state. A returning peer catches up through log replay
//! or a snapshot containing both metadata and immutable CAS content.
use crate::{
    chunk::{blake3_digest, ChunkRef, CHUNK_SIZE},
    engine::MeshFSEngine,
    merkle::Inode,
    volume::{Snapshot as FileSnapshot, Volume},
};
use openraft::error::{
    InstallSnapshotError, NetworkError, PayloadTooLarge, RPCError, RaftError, RemoteError,
};
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
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fmt::Debug,
    io::{self, Read, Seek, SeekFrom, Write},
    ops::{Bound, RangeBounds},
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

pub const MAX_INLINE_BYTES: usize = 16 * 1024 * 1024;
const REPLAY_EPOCH_ADMISSIONS: u64 = 256;
// Up to 1 TiB of distinct canonical chunks may await a file manifest.
const RECENT_STAGED_BLOBS: usize = 262_144;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    /// A replicated read, outside replay admission and deduplication.
    ReplayEpoch,
    Live(crate::live::Commit),
    /// Replicate one durable immutable block before publishing file metadata.
    StageChunk {
        hex: String,
    },
    CommitFile {
        volume: String,
        inode: Inode,
    },
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
    #[serde(default)]
    pub epoch: u64,
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

// Opt-in diagnostics never write on Raft/Tokio threads. Correlation is an
// additional opt-in; it shares the same 256-slot queue and 10,000-event budget.
#[derive(Clone, Copy, Debug, Default)]
struct Correlation {
    command: Option<[u8; 32]>,
    batch: u64,
    first: [u64; 3], // term, leader node, index (inclusive)
    last: [u64; 3],
    entry: Option<[u64; 3]>,
}
#[derive(Clone, Copy, Debug, Default)]
struct DiagnosticStamp {
    monotonic_ns: u64, // Linux CLOCK_MONOTONIC, common to this guest's processes
    tid: u64,          // OS TID, never Rust ThreadId
}
fn diagnostic_stamp() -> DiagnosticStamp {
    #[cfg(target_os = "linux")]
    {
        // No I/O or heap allocation. Errors are explicitly unavailable (zero).
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        let ns = if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } == 0 {
            (ts.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec as u64)
        } else { 0 };
        let tid = unsafe { libc::syscall(libc::SYS_gettid) };
        DiagnosticStamp { monotonic_ns: ns, tid: if tid > 0 { tid as u64 } else { 0 } }
    }
    #[cfg(not(target_os = "linux"))]
    { DiagnosticStamp::default() }
}
fn opaque_command(key: &[u8; 32], command: &Command) -> Option<[u8; 32]> {
    if command.id.is_empty() || command.id.len() > 256 { return None; }
    let mut h = blake3::Hasher::new();
    h.update(b"meshfs-diagnostic-command-v1\0");
    h.update(key);
    h.update(&command.epoch.to_be_bytes());
    h.update(&(command.id.len() as u64).to_be_bytes());
    h.update(command.id.as_bytes());
    Some(*h.finalize().as_bytes())
}
fn diagnostic_log_id(id: LogId<u64>) -> [u64; 3] {
    [id.leader_id.term, id.leader_id.node_id, id.index]
}
struct SlowDiagnostics {
    sender: std::sync::mpsc::SyncSender<SlowEvent>,
    start: std::time::Instant,
    threshold: std::time::Duration,
    attempted: Arc<std::sync::atomic::AtomicU64>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
    capped: Arc<std::sync::atomic::AtomicU64>,
    next_span: std::sync::atomic::AtomicU64,
    correlation_key: Option<[u8; 32]>,
}
struct SlowEvent {
    operation: &'static str,
    phase: &'static str,
    start_us: u64,
    elapsed_us: u64,
    thread: std::thread::ThreadId,
    sequence: u64,
    dropped: u64,
    count: u64,
    correlation: Option<Correlation>,
    span: u64,
    start_stamp: DiagnosticStamp,
    end_stamp: DiagnosticStamp,
    outcome: u8, // 0=unspecified/cancelled/error, 1=client OK, 2=Raft error, 3=timeout
}
fn write_slow_event(output: &mut impl Write, event: &SlowEvent, totals: [u64; 3]) -> io::Result<()> {
    let c = event.correlation.unwrap_or_default();
    let cmd = c.command.map(hex::encode).unwrap_or_else(|| "none".into());
    let entry = c.entry.unwrap_or([0; 3]);
    writeln!(output,
        "meshfs_slow_phase seq={} op={} phase={} start_us={} elapsed_us={} thread={:?} dropped={} count={} correlated={} cmd={} batch={} first_term={} first_node={} first_index={} last_term={} last_node={} last_index={} entry_present={} entry_term={} entry_node={} entry_index={} span={} pid={} clock=linux_monotonic start_ns={} end_ns={} start_tid={} end_tid={} outcome={} attempted={} dropped_now={} capped={}",
        event.sequence, event.operation, event.phase, event.start_us, event.elapsed_us, event.thread,
        event.dropped, event.count, u8::from(event.correlation.is_some()), cmd, c.batch,
        c.first[0], c.first[1], c.first[2], c.last[0], c.last[1], c.last[2],
        u8::from(c.entry.is_some()), entry[0], entry[1], entry[2], event.span, std::process::id(),
        event.start_stamp.monotonic_ns, event.end_stamp.monotonic_ns, event.start_stamp.tid,
        event.end_stamp.tid, event.outcome, totals[0], totals[1], totals[2])
}
impl SlowDiagnostics {
    fn emit(&self, mut event: SlowEvent) {
        use std::sync::atomic::Ordering::Relaxed;
        let sequence = match self.attempted.fetch_update(Relaxed, Relaxed, |v| (v < 10_000).then_some(v + 1)) {
            Ok(v) => v,
            Err(_) => { self.capped.fetch_add(1, Relaxed); return; }
        };
        event.sequence = sequence;
        event.dropped = self.dropped.load(Relaxed);
        if self.sender.try_send(event).is_err() { self.dropped.fetch_add(1, Relaxed); }
    }
    fn command(&self, command: &Command) -> Option<Correlation> {
        let key = self.correlation_key.as_ref()?;
        Some(Correlation { command: Some(opaque_command(key, command)?), ..Correlation::default() })
    }
    fn entry(&self, mut batch: Correlation, entry: &Entry<Types>) -> Correlation {
        batch.entry = Some(diagnostic_log_id(entry.log_id));
        batch.command = match &entry.payload {
            EntryPayload::Normal(c) => self.command(c).and_then(|v| v.command),
            _ => None,
        };
        batch
    }
    fn batch(&self, entries: &[Entry<Types>]) -> Option<Correlation> {
        self.correlation_key?;
        Some(Correlation {
            batch: self.next_span.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            first: diagnostic_log_id(entries.first()?.log_id),
            last: diagnostic_log_id(entries.last()?.log_id),
            ..Correlation::default()
        })
    }
}
fn slow_diagnostics() -> Option<&'static SlowDiagnostics> {
    static DIAG: std::sync::OnceLock<Option<SlowDiagnostics>> = std::sync::OnceLock::new();
    DIAG.get_or_init(|| {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        let millis = std::env::var("MESHFS_SLOW_PHASE_MS").ok()?.parse::<u64>().ok()?;
        if !(1..=60_000).contains(&millis) { return None; }
        // Same cluster token yields the same opaque command key on each peer.
        // Token/key bytes, command IDs, actions and payloads never enter events.
        let correlation_requested = std::env::var("MESHFS_CORRELATION").as_deref() == Ok("1");
        let correlation_key = if correlation_requested {
            std::env::var("MESHFS_CLUSTER_TOKEN").ok().filter(|v| !v.is_empty())
                .map(|v| *blake3::hash(&[b"meshfs-diagnostic-key-v1\0".as_slice(), v.as_bytes()].concat()).as_bytes())
        } else { None };
        let (sender, receiver) = std::sync::mpsc::sync_channel::<SlowEvent>(256);
        let attempted = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let capped = Arc::new(AtomicU64::new(0));
        let counters = (attempted.clone(), dropped.clone(), capped.clone());
        let correlation_status = if correlation_key.is_some() { "enabled" }
            else if correlation_requested { "disabled_no_token" } else { "disabled_not_requested" };
        std::thread::Builder::new().name("meshfs-slow-phases".into()).spawn(move || {
            let mut output = std::io::stderr();
            // One key-free configuration line, from the writer only.
            if writeln!(output, "meshfs_diagnostic_config correlation={} clock=linux_monotonic queue=256 max_events=10000", correlation_status).is_err() { return; }
            let mut cap_reported = false;
            loop {
                match receiver.recv_timeout(std::time::Duration::from_millis(250)) {
                    Ok(event) => {
                        if write_slow_event(&mut output, &event, [counters.0.load(Relaxed), counters.1.load(Relaxed), counters.2.load(Relaxed)]).is_err() { break; }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                // One extra bounded summary makes exhaustion visible even if the
                // final attempt was dropped. Counters are snapshots/lower bounds.
                if !cap_reported && counters.0.load(Relaxed) >= 10_000 {
                    if writeln!(output, "meshfs_diagnostic_cap exhausted=1 attempted={} dropped={} capped_at_least={}", counters.0.load(Relaxed), counters.1.load(Relaxed), counters.2.load(Relaxed)).is_err() { break; }
                    cap_reported = true;
                }
            }
        }).ok()?;
        Some(SlowDiagnostics { sender, start: std::time::Instant::now(), threshold: std::time::Duration::from_millis(millis),
            attempted, dropped, capped, next_span: AtomicU64::new(1), correlation_key })
    }).as_ref()
}
struct SlowPhase {
    diagnostic: Option<&'static SlowDiagnostics>,
    operation: &'static str,
    phase: &'static str,
    start: Option<std::time::Instant>,
    minimum: std::time::Duration,
    count: u64,
    correlation: Option<Correlation>,
    span: u64,
    start_stamp: DiagnosticStamp,
    outcome: u8,
}
impl SlowPhase {
    fn new(operation: &'static str, phase: &'static str) -> Self {
        let diagnostic = slow_diagnostics();
        Self { diagnostic, operation, phase, start: diagnostic.map(|_| std::time::Instant::now()), minimum: std::time::Duration::ZERO, count: 0,
            correlation: None, span: 0, start_stamp: DiagnosticStamp::default(), outcome: 0 }
    }
    fn with_count(mut self, count: usize) -> Self { self.count = count as u64; self }
    fn with_minimum(mut self, minimum: std::time::Duration) -> Self { self.minimum = minimum; self }
    fn with_correlation(mut self, correlation: Option<Correlation>) -> Self {
        self.correlation = correlation;
        if correlation.is_some() {
            self.start_stamp = diagnostic_stamp();
            if let Some(d) = self.diagnostic { self.span = d.next_span.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
        }
        self
    }
}
impl Drop for SlowPhase {
    fn drop(&mut self) {
        if let (Some(diagnostic), Some(start)) = (self.diagnostic, self.start) {
            let elapsed = start.elapsed();
            // Correlated join spans must survive even when faster than the slow
            // threshold. Budget/drop/cancellation still preclude completeness.
            if self.correlation.is_none() && elapsed < diagnostic.threshold.max(self.minimum) { return; }
            diagnostic.emit(SlowEvent {
                operation: self.operation, phase: self.phase,
                start_us: start.duration_since(diagnostic.start).as_micros().min(u64::MAX as u128) as u64,
                elapsed_us: elapsed.as_micros().min(u64::MAX as u128) as u64,
                thread: std::thread::current().id(), sequence: 0, dropped: 0, count: self.count,
                correlation: self.correlation, span: self.span, start_stamp: self.start_stamp,
                end_stamp: if self.correlation.is_some() { diagnostic_stamp() } else { DiagnosticStamp::default() }, outcome: self.outcome,
            });
        }
    }
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
        blake3_digest(
            &serde_json::to_vec(&("meshfs-live-v3-epochs", &self.cluster, &self.peers))
                .expect("serializable topology"),
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
    log_io: Arc<Semaphore>,
}
impl Db {
    fn open(path: &Path) -> io::Result<Self> {
        // Initialize the optional writer before Raft starts polling storage.
        let _ = slow_diagnostics();
        let c = Connection::open(path).map_err(err)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY,v TEXT NOT NULL); CREATE TABLE IF NOT EXISTS logs(idx INTEGER PRIMARY KEY,entry TEXT NOT NULL); CREATE TABLE IF NOT EXISTS machine_deltas(last_idx INTEGER PRIMARY KEY,entries TEXT NOT NULL);").map_err(err)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(c)),
            log_io: Arc::new(Semaphore::new(1)),
        })
    }
    // One shared admission lane across storage/read clones. The permit lives
    // inside the blocking task, so caller cancellation cannot release the lane
    // while its SQLite work (or durable-completion callback) is still running.
    async fn log_io<T, F>(&self, operation: &'static str, work: F) -> SResult<T>
    where
        T: Send + 'static,
        F: FnOnce(Db) -> SResult<T> + Send + 'static,
    {
        let admission = SlowPhase::new(operation, "admission_wait");
        let permit = self.log_io.clone().acquire_owned().await.map_err(se)?;
        drop(admission);
        let queued = SlowPhase::new(operation, "blocking_pool_wait");
        let db = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            drop(queued);
            let _work = SlowPhase::new(operation, "blocking_work");
            work(db)
        })
        .await
        .map_err(se)?
    }
    fn lock_conn(&self, operation: &'static str) -> SResult<std::sync::MutexGuard<'_, Connection>> {
        let _wait = SlowPhase::new(operation, "db_mutex_wait");
        self.conn.lock().map_err(se)
    }
    fn get<T: DeserializeOwned>(&self, k: &str) -> SResult<Option<T>> {
        let c = self.lock_conn("get")?;
        let _sql = SlowPhase::new("meta_get", "sql");
        let s: Option<String> = c
            .query_row("SELECT v FROM meta WHERE k=?", [k], |r| r.get(0))
            .optional()
            .map_err(se)?;
        s.map(|s| serde_json::from_str(&s).map_err(se)).transpose()
    }
    fn set<T: Serialize>(&self, k: &str, v: &T) -> SResult<()> {
        let c = self.lock_conn("meta_set")?;
        let _sql = SlowPhase::new("meta_set", "sql");
        c.execute(
            "INSERT INTO meta(k,v) VALUES(?,?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            params![k, serde_json::to_string(v).map_err(se)?],
        )
        .map_err(se)?;
        Ok(())
    }
    fn persist_apply(&self, entries: &[Entry<Types>], applied: Option<LogId<u64>>, correlation: Option<Correlation>) -> SResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let serialize = SlowPhase::new("persist_apply", "serialize").with_correlation(correlation);
        let raw = serde_json::to_string(entries).map_err(se)?;
        let marker = serde_json::to_string(&applied).map_err(se)?;
        let last = entries.last().expect("nonempty batch").log_id.index;
        drop(serialize);
        let lock_wait = correlation.map(|ctx| SlowPhase::new("persist_apply", "db_mutex_wait").with_correlation(Some(ctx)));
        let mut c = self.lock_conn("persist_apply")?;
        drop(lock_wait);
        let sql = SlowPhase::new("persist_apply", "sql").with_correlation(correlation);
        let tx = c.transaction().map_err(se)?;
        tx.execute(
            "INSERT INTO machine_deltas(last_idx,entries) VALUES(?,?)",
            params![last, raw],
        )
        .map_err(se)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('machine_applied',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [marker]).map_err(se)?;
        drop(sql);
        let _commit = SlowPhase::new("persist_apply", "commit").with_correlation(correlation);
        tx.commit().map_err(se)?;
        Ok(())
    }
    fn checkpoint(&self, state: &MachineData, install: bool) -> SResult<()> {
        let serialize = SlowPhase::new("checkpoint", "serialize");
        let raw = serde_json::to_string(state).map_err(se)?;
        let marker = serde_json::to_string(&state.applied).map_err(se)?;
        drop(serialize);
        let mut c = self.lock_conn("checkpoint")?;
        let sql = SlowPhase::new("checkpoint", "sql");
        let tx = c.transaction().map_err(se)?;
        let base: Option<String> = tx
            .query_row("SELECT v FROM meta WHERE k='machine_base'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(se)?;
        let base_index = match base {
            Some(v) => serde_json::from_str::<Option<LogId<u64>>>(&v)
                .map_err(se)?
                .map(|v| v.index),
            None => {
                // One-time legacy fallback; subsequent checkpoints compare only a small marker.
                let raw: Option<String> = tx
                    .query_row("SELECT v FROM meta WHERE k='machine'", [], |r| r.get(0))
                    .optional()
                    .map_err(se)?;
                raw.map(|v| serde_json::from_str::<MachineData>(&v).map_err(se))
                    .transpose()?
                    .and_then(|v| v.applied)
                    .map(|v| v.index)
            }
        };
        if !install && base_index > state.applied.map(|v| v.index) {
            return Ok(());
        }
        tx.execute(
            "INSERT INTO meta(k,v) VALUES('machine',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            [raw],
        )
        .map_err(se)?;
        tx.execute("INSERT INTO meta(k,v) VALUES('machine_base',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [&marker]).map_err(se)?;
        let latest: Option<String> = tx
            .query_row("SELECT v FROM meta WHERE k='machine_applied'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(se)?;
        let latest = latest
            .map(|v| serde_json::from_str::<Option<LogId<u64>>>(&v).map_err(se))
            .transpose()?
            .flatten();
        if install || latest.map(|v| v.index) <= state.applied.map(|v| v.index) {
            tx.execute("INSERT INTO meta(k,v) VALUES('machine_applied',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v", [marker]).map_err(se)?;
        }
        if install {
            tx.execute("DELETE FROM machine_deltas", []).map_err(se)?;
        } else if let Some(id) = state.applied {
            tx.execute("DELETE FROM machine_deltas WHERE last_idx<=?", [id.index])
                .map_err(se)?;
        }
        drop(sql);
        let _commit = SlowPhase::new("checkpoint", "commit");
        tx.commit().map_err(se)?;
        Ok(())
    }
}
impl RaftLogReader<Types> for Db {
    async fn try_get_log_entries<R: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: R,
    ) -> SResult<Vec<Entry<Types>>> {
        let start = match range.start_bound() {
            Bound::Included(v) => *v,
            Bound::Excluded(v) => match v.checked_add(1) {
                Some(v) => v,
                None => return Ok(vec![]),
            },
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(v) => *v,
            Bound::Excluded(v) => match v.checked_sub(1) {
                Some(v) => v,
                None => return Ok(vec![]),
            },
            Bound::Unbounded => i64::MAX as u64,
        }
        .min(i64::MAX as u64);
        if start > end {
            return Ok(vec![]);
        }
        self.log_io("try_get_log_entries", move |db| {
            let c = db.lock_conn("try_get_log_entries")?;
            let _sql = SlowPhase::new("try_get_log_entries", "sql");
            let mut q = c
                .prepare("SELECT entry FROM logs WHERE idx>=? AND idx<=? ORDER BY idx")
                .map_err(se)?;
            let rows = q
                .query_map(params![start, end], |r| r.get::<_, String>(0))
                .map_err(se)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(serde_json::from_str(&row.map_err(se)?).map_err(se)?);
            }
            Ok(out)
        })
        .await
    }
}
impl RaftLogStorage<Types> for Db {
    type LogReader = Self;
    async fn get_log_state(&mut self) -> SResult<LogState<Types>> {
        self.log_io("get_log_state", move |db| {
            let purged: Option<LogId<u64>> = db.get("purged")?;
            let c = db.lock_conn("get_log_state")?;
            let _sql = SlowPhase::new("get_log_state", "sql");
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
        })
        .await
    }
    async fn get_log_reader(&mut self) -> Self {
        // No connection access; the reader clone shares the admission lane.
        self.clone()
    }
    async fn save_vote(&mut self, v: &Vote<u64>) -> SResult<()> {
        let v = v.clone();
        self.log_io("save_vote", move |db| db.set("vote", &v)).await
    }
    async fn read_vote(&mut self) -> SResult<Option<Vote<u64>>> {
        self.log_io("read_vote", move |db| db.get("vote")).await
    }
    async fn save_committed(&mut self, v: Option<LogId<u64>>) -> SResult<()> {
        self.log_io("save_committed", move |db| db.set("committed", &v)).await
    }
    async fn read_committed(&mut self) -> SResult<Option<LogId<u64>>> {
        self.log_io("read_committed", move |db| {
            Ok(db.get::<Option<LogId<u64>>>("committed")?.flatten())
        })
        .await
    }
    async fn append<I>(&mut self, entries: I, callback: LogFlushed<Types>) -> SResult<()>
    where
        I: IntoIterator<Item = Entry<Types>> + Send,
        I::IntoIter: Send,
    {
        // Trait iterators need not be 'static; move owned entries into the task.
        let entries: Vec<_> = entries.into_iter().collect();
        self.log_io("append", move |db| {
            {
                let mut c = db.lock_conn("append")?;
                let sql = SlowPhase::new("append", "sql");
                let tx = c.transaction().map_err(se)?;
                for e in entries {
                    tx.execute("INSERT INTO logs(idx,entry) VALUES(?,?) ON CONFLICT(idx) DO UPDATE SET entry=excluded.entry",params![e.log_id.index,serde_json::to_string(&e).map_err(se)?]).map_err(se)?;
                }
                drop(sql);
                let _commit = SlowPhase::new("append", "commit");
                tx.commit().map_err(se)?;
            }
            // Keep this with the commit: dropping the awaiting future must not
            // discard the notification after a successful durable append.
            callback.log_io_completed(Ok(()));
            Ok(())
        })
        .await
    }
    async fn truncate(&mut self, id: LogId<u64>) -> SResult<()> {
        self.log_io("truncate", move |db| {
            let c = db.lock_conn("truncate")?;
            let _sql = SlowPhase::new("truncate", "sql");
            c.execute("DELETE FROM logs WHERE idx>=?", [id.index])
                .map_err(se)?;
            Ok(())
        })
        .await
    }
    async fn purge(&mut self, id: LogId<u64>) -> SResult<()> {
        self.log_io("purge", move |db| {
            let mut c = db.lock_conn("purge")?;
            let sql = SlowPhase::new("purge", "sql");
            let tx = c.transaction().map_err(se)?;
            tx.execute("DELETE FROM logs WHERE idx<=?", [id.index])
                .map_err(se)?;
            tx.execute(
                "INSERT INTO meta(k,v) VALUES('purged',?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                [serde_json::to_string(&id).map_err(se)?],
            )
            .map_err(se)?;
            drop(sql);
            let _commit = SlowPhase::new("purge", "commit");
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct MachineData {
    applied: Option<LogId<u64>>,
    membership: StoredMembership<u64, BasicNode>,
    volumes: HashMap<String, Volume>,
    snapshots: Vec<FileSnapshot>,
    seen: BTreeMap<String, (String, Value)>,
    // Metadata retains active/snapshot references and bounded recent staging.
    // Physical CAS deletion is deliberately separate from replay admission.
    #[serde(default)]
    blobs: BTreeMap<String, usize>,
    #[serde(default)]
    live_ops: BTreeMap<String, (String, Value)>,
    #[serde(default)]
    live_revision: u64,
    #[serde(default)]
    replay_epoch: u64,
    #[serde(default)]
    replay_admissions: u64,
    #[serde(default)]
    staging_admissions: u64,
    #[serde(default)]
    recent_staging: VecDeque<String>,
    #[serde(default)]
    seen_epochs: BTreeMap<String, u64>,
    #[serde(default)]
    live_op_epochs: BTreeMap<String, u64>,
}
pub struct Machine {
    db: Db,
    engine: MeshFSEngine,
    data: AsyncMutex<MachineData>,
}
impl Machine {
    fn restored_state(&self) -> SResult<MachineData> {
        // One consistent SQLite view of the legacy/full checkpoint and tail.
        let (mut state, batches, marker) = {
            let c = self.db.conn.lock().map_err(se)?;
            let raw: Option<String> = c
                .query_row("SELECT v FROM meta WHERE k='machine'", [], |r| r.get(0))
                .optional()
                .map_err(se)?;
            let state = raw
                .map(|v| serde_json::from_str::<MachineData>(&v).map_err(se))
                .transpose()?
                .unwrap_or_default();
            let mut q = c
                .prepare("SELECT last_idx,entries FROM machine_deltas ORDER BY last_idx")
                .map_err(se)?;
            let rows = q
                .query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)))
                .map_err(se)?;
            let batches = rows.collect::<Result<Vec<_>, _>>().map_err(se)?;
            let marker: Option<String> = c
                .query_row("SELECT v FROM meta WHERE k='machine_applied'", [], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(se)?;
            (state, batches, marker)
        };
        for (last, raw) in batches {
            let entries: Vec<Entry<Types>> = serde_json::from_str(&raw).map_err(se)?;
            if entries.is_empty() || entries.last().map(|v| v.log_id.index) != Some(last) {
                return Err(se("invalid durable apply batch"));
            }
            for e in entries {
                if state.applied.is_some_and(|id| e.log_id.index <= id.index) {
                    return Err(se(
                        "durable apply journal overlaps checkpoint or is unordered",
                    ));
                }
                if state
                    .applied
                    .is_some_and(|id| id.index.checked_add(1) != Some(e.log_id.index))
                {
                    return Err(se("durable apply journal has a gap"));
                }
                match e.payload {
                    EntryPayload::Blank => {}
                    EntryPayload::Membership(m) => {
                        state.membership = StoredMembership::new(Some(e.log_id), m)
                    }
                    EntryPayload::Normal(c) => {
                        self.execute(&mut state, &c)?;
                    }
                }
                state.applied = Some(e.log_id);
            }
        }
        if let Some(marker) = marker {
            let expected: Option<LogId<u64>> = serde_json::from_str(&marker).map_err(se)?;
            if state.applied != expected {
                return Err(se("durable apply marker differs from reconstructed state"));
            }
        }
        Ok(state)
    }
    async fn mirror(&self, data: &MachineData) {
        let wait = SlowPhase::new("mirror", "engine_state_wait");
        let mut s = self.engine.state.write().await;
        drop(wait);
        let _clone = SlowPhase::new("mirror", "clone_metadata");
        s.volumes = data.volumes.clone();
        s.snapshots = data.snapshots.clone();
    }
    fn execute(&self, s: &mut MachineData, c: &Command) -> SResult<Value> {
        if matches!(&c.action, Action::ReplayEpoch) {
            return Ok(json!({"epoch":s.replay_epoch}));
        }
        // Fence before either command-ID or live-operation deduplication. A
        // forgotten operation can never acquire authority in a newer epoch.
        if (c.epoch != s.replay_epoch && c.epoch.checked_add(1) != Some(s.replay_epoch))
            || matches!(&c.action, Action::Live(change) if change.epoch != c.epoch)
        {
            return Ok(json!({"error":"live replay epoch expired"}));
        }
        // Immutable CAS staging must not age the mutable operation whose
        // potentially large inode is still being assembled. Its metadata has
        // its own bounded cleanup cadence, without mutable replay authority.
        if matches!(&c.action, Action::StageChunk { .. }) {
            let result = self.execute_action(s, c)?;
            s.staging_admissions += 1;
            if s.staging_admissions >= REPLAY_EPOCH_ADMISSIONS {
                s.staging_admissions = 0;
                Self::prune_blob_metadata(s);
            }
            return Ok(result);
        }
        let digest = blake3_digest(&serde_json::to_vec(&c.action).map_err(se)?);
        if let Some((previous, result)) = s.seen.get(&c.id) {
            return Ok(if previous == &digest {
                result.clone()
            } else {
                json!({"error":"operation ID reused for different content"})
            });
        }
        // Refuse before applying anything if the final epoch cannot rotate.
        if s.replay_admissions >= REPLAY_EPOCH_ADMISSIONS - 1 && s.replay_epoch == u64::MAX {
            return Ok(json!({"error":"live replay epoch exhausted"}));
        }
        let result = self.execute_action(s, c)?;
        s.replay_admissions += 1;
        if s.replay_admissions >= REPLAY_EPOCH_ADMISSIONS {
            s.replay_epoch = s
                .replay_epoch
                .checked_add(1)
                .expect("epoch checked before admission");
            s.replay_admissions = 0;
            let oldest = s.replay_epoch - 1;
            s.seen
                .retain(|id, _| s.seen_epochs.get(id).copied().unwrap_or(0) >= oldest);
            s.seen_epochs.retain(|id, _| s.seen.contains_key(id));
            s.live_ops
                .retain(|id, _| s.live_op_epochs.get(id).copied().unwrap_or(0) >= oldest);
            s.live_op_epochs.retain(|id, _| s.live_ops.contains_key(id));
            Self::prune_blob_metadata(s);
        }
        Ok(result)
    }
    fn prune_blob_metadata(s: &mut MachineData) {
        let _prune = SlowPhase::new("stage_metadata", "prune").with_count(s.blobs.len());
        let mut retained: BTreeSet<String> = s.recent_staging.iter().cloned().collect();
        for inode in s
            .volumes
            .values()
            .flat_map(|v| v.files.values())
            .chain(s.snapshots.iter().flat_map(|v| v.files_tree.values()))
        {
            retained.extend(inode.chunks.iter().map(|c| c.id.clone()));
        }
        s.blobs.retain(|id, _| retained.contains(id));
    }
    fn execute_action(&self, s: &mut MachineData, c: &Command) -> SResult<Value> {
        let digest = blake3_digest(&serde_json::to_vec(&c.action).map_err(se)?);
        if !matches!(&c.action, Action::StageChunk { .. }) {
            if let Some((previous, result)) = s.seen.get(&c.id) {
                return Ok(if previous == &digest {
                    result.clone()
                } else {
                    json!({"error":"operation ID reused for different content"})
                });
            }
        }
        let result = match &c.action {
            Action::ReplayEpoch => unreachable!("epoch reads bypass admission"),
            Action::Live(change) => {
                if let Some((hash, result)) = s.live_ops.get(&change.id) {
                    return Ok(if hash == &change.hash {
                        result.clone()
                    } else {
                        json!({"error":"live operation ID reused"})
                    });
                }
                if change.probe {
                    return Ok(json!({"error":"live compare conflict"}));
                }
                let current = s
                    .volumes
                    .get(&change.volume)
                    .and_then(|v| v.files.get(&change.path));
                if current.map(crate::live::version) != change.expected {
                    return Ok(json!({"error":"live compare conflict"}));
                }
                if let Some(to) = &change.rename {
                    let target = s.volumes.get(&change.volume).and_then(|v| v.files.get(to));
                    if change.destination.as_ref().map(|p| p.version.clone())
                        != Some(target.map(crate::live::version))
                    {
                        return Ok(json!({"error":"live compare conflict at rename destination"}));
                    }
                }
                if let Some(inode) = &change.inode {
                    if let Err(e) = crate::live::validate(inode, &s.blobs) {
                        return Ok(json!({"error":e.to_string()}));
                    }
                }
                let Some(revision) = s.live_revision.checked_add(1) else {
                    return Ok(json!({"error":"live revision exhausted"}));
                };
                s.live_revision = revision;
                let v = s
                    .volumes
                    .entry(change.volume.clone())
                    .or_insert_with(|| Volume::new(&change.volume));
                if let Some(to) = &change.rename {
                    let Some(mut inode) = v.files.remove(&change.path) else {
                        return Ok(json!({"error":"rename source missing"}));
                    };
                    inode.revision = revision;
                    inode.path = to.clone();
                    v.files.insert(to.clone(), inode);
                } else if let Some(inode) = &change.inode {
                    let mut inode = inode.clone();
                    inode.revision = revision;
                    v.files.insert(change.path.clone(), inode);
                } else {
                    v.files.remove(&change.path);
                }
                v.recompute_merkle();
                let result = json!({"ok":true});
                s.live_ops
                    .insert(change.id.clone(), (change.hash.clone(), result.clone()));
                s.live_op_epochs.insert(change.id.clone(), c.epoch);
                result
            }
            Action::StageChunk { hex } => {
                let bytes = hex::decode(hex).map_err(se)?;
                if bytes.len() > CHUNK_SIZE {
                    return Err(se("oversized staged chunk"));
                }
                let write = SlowPhase::new("stage_chunk", "cas_write").with_count(1);
                let id = self.engine.chunk_store.write_chunk(&bytes).map_err(se)?;
                drop(write);
                let _metadata = SlowPhase::new("stage_chunk", "metadata").with_count(1);
                let existed = s.blobs.insert(id.clone(), bytes.len()).is_some();
                if existed {
                    s.recent_staging.retain(|previous| previous != &id);
                }
                s.recent_staging.push_back(id.clone());
                while s.recent_staging.len() > RECENT_STAGED_BLOBS {
                    s.recent_staging.pop_front();
                }
                return Ok(json!(id));
            }
            Action::CommitFile { volume, inode } => {
                if s.volumes
                    .get(volume)
                    .and_then(|v| v.files.get(&inode.path))
                    .is_some_and(|i| i.revision > 0 || i.digest.starts_with("merkle-blake3:"))
                {
                    return Ok(json!({"error":"live file requires precondition"}));
                }
                if let Err(e) = validate_layout(inode, &s.blobs) {
                    return Ok(json!({"error":e.to_string()}));
                }
                let v = s.volumes.entry(volume.clone()).or_insert_with(|| {
                    let mut v = Volume::new(volume);
                    v.created_at = inode.updated_at.clone();
                    v
                });
                v.insert_file(inode.clone());
                json!(inode)
            }
            Action::Put {
                volume,
                path,
                hex,
                at,
            } => {
                if s.volumes
                    .get(volume)
                    .and_then(|v| v.files.get(path))
                    .is_some_and(|i| i.revision > 0 || i.digest.starts_with("merkle-blake3:"))
                {
                    return Ok(json!({"error":"live file requires precondition"}));
                }
                let bytes = hex::decode(hex).map_err(se)?;
                let mut chunks = Vec::new();
                let mut offset = 0;
                for part in bytes.chunks(CHUNK_SIZE) {
                    let id = self.engine.chunk_store.write_chunk(part).map_err(se)?;
                    s.blobs.insert(id.clone(), part.len());
                    chunks.push(ChunkRef {
                        id,
                        size: part.len(),
                        offset,
                    });
                    offset += part.len() as u64;
                }
                let inode = Inode {
                    revision: 0,
                    path: path.clone(),
                    size: bytes.len() as u64,
                    digest: format!("blake3:{}", blake3_digest(&bytes)),
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
        s.seen_epochs.insert(c.id.clone(), c.epoch);
        Ok(result)
    }
    fn snapshot_file(&self) -> SResult<std::fs::File> {
        let dir = self.engine.data_dir.join("raft/snapshots");
        std::fs::create_dir_all(&dir).map_err(se)?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = dir.join(format!(
            "transfer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(se)?;
        // OpenRaft owns the descriptor until transfer completion. Unlinking the
        // temporary name also removes abandoned transfers on process termination.
        std::fs::remove_file(path).map_err(se)?;
        Ok(file)
    }
    fn chunk_ids(s: &MachineData) -> BTreeSet<String> {
        let mut ids: BTreeSet<String> = s.blobs.keys().cloned().collect();
        for inode in s
            .volumes
            .values()
            .flat_map(|v| v.files.values())
            .chain(s.snapshots.iter().flat_map(|v| v.files_tree.values()))
        {
            for chunk in &inode.chunks {
                ids.insert(chunk.id.clone());
            }
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
        let machine = self.clone();
        let correlation = slow_diagnostics().and_then(|d| d.batch(&entries));
        let queued = SlowPhase::new("apply", "blocking_pool_wait").with_count(entries.len()).with_correlation(correlation);
        // Raft awaits this ordered batch. CAS I/O, serialization and the FULL
        // SQLite commit must finish before acknowledgement, but must not block
        // Tokio's RPC/election/timer workers while doing so.
        let out = tokio::task::spawn_blocking(move || {
            drop(queued);
            let state_wait = SlowPhase::new("apply", "state_mutex_wait").with_count(entries.len()).with_correlation(correlation);
            let mut s = machine.data.blocking_lock();
            drop(state_wait);
            let execute = SlowPhase::new("apply", "execute_batch").with_count(entries.len()).with_correlation(correlation);
            let mut out = Vec::with_capacity(entries.len());
            for e in &entries {
                let entry_correlation = correlation.and_then(|ctx| slow_diagnostics().map(|d| d.entry(ctx, e)));
                let _entry = SlowPhase::new("apply", "execute_entry").with_count(1).with_correlation(entry_correlation);
                let value = match &e.payload {
                    EntryPayload::Blank => Value::Null,
                    EntryPayload::Membership(m) => {
                        s.membership = StoredMembership::new(Some(e.log_id), m.clone());
                        Value::Null
                    }
                    EntryPayload::Normal(c) => machine.execute(&mut s, &c)?,
                };
                s.applied = Some(e.log_id);
                out.push(value);
            }
            drop(execute);
            let _persist = SlowPhase::new("apply", "persist_total").with_count(entries.len()).with_correlation(correlation);
            machine.db.persist_apply(&entries, s.applied, correlation)?;
            Ok::<_, StorageError<u64>>(out)
        })
        .await
        .map_err(se)??;
        let state_wait = SlowPhase::new("apply", "mirror_state_wait").with_count(out.len()).with_correlation(correlation);
        let s = self.data.lock().await;
        drop(state_wait);
        let _mirror = SlowPhase::new("apply", "mirror_total").with_count(out.len()).with_correlation(correlation);
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
        let mut file = (*snapshot).into_std().await;
        file.sync_all().map_err(se)?;
        file.seek(SeekFrom::Start(0)).map_err(se)?;
        let mut reader = std::io::BufReader::new(file);
        let mut checksum = [0u8; 32];
        reader.read_exact(&mut checksum).map_err(se)?;
        let metadata = read_frame(&mut reader, 128 * 1024 * 1024)?;
        if hex::encode(checksum) != blake3_digest(&metadata) {
            return Err(se("snapshot metadata checksum mismatch"));
        }
        let state: MachineData = serde_json::from_slice(&metadata).map_err(se)?;
        if state.applied != meta.last_log_id || state.membership != meta.last_membership {
            return Err(se("snapshot metadata mismatch"));
        }
        let expected = Machine::chunk_ids(&state);
        let mut sizes = BTreeMap::new();
        for id in &expected {
            let bytes = read_frame(&mut reader, CHUNK_SIZE)?;
            sizes.insert(id.clone(), bytes.len());
            if self.engine.chunk_store.write_chunk(&bytes).map_err(se)? != *id {
                return Err(se("snapshot chunk hash mismatch"));
            }
        }
        let mut extra = [0u8; 1];
        if reader.read(&mut extra).map_err(se)? != 0 {
            return Err(se("snapshot trailing bytes"));
        }
        for inode in state
            .volumes
            .values()
            .flat_map(|v| v.files.values())
            .chain(state.snapshots.iter().flat_map(|v| v.files_tree.values()))
        {
            validate_layout(inode, &sizes)?;
        }
        let mut s = self.data.lock().await;
        self.db.checkpoint(&state, true)?;
        *s = state;
        self.mirror(&s).await;
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
        let machine = self.clone();
        tokio::task::spawn_blocking(move || {
            // Clone one consistent applied state off Tokio; release its lock
            // before taking the SQLite connection or streaming CAS content.
            let state_wait = SlowPhase::new("snapshot", "state_mutex_wait");
            let guard = machine.data.blocking_lock();
            drop(state_wait);
            let clone = SlowPhase::new("snapshot", "clone");
            let state = guard.clone();
            drop(guard);
            drop(clone);
            let meta = SnapshotMeta {
                last_log_id: state.applied,
                last_membership: state.membership.clone(),
                snapshot_id: format!("{:?}", state.applied),
            };
            let mut file = machine.snapshot_file()?;
            let checkpoint = SlowPhase::new("snapshot", "checkpoint_total");
            machine.db.checkpoint(&state, false)?;
            drop(checkpoint);
            let _stream = SlowPhase::new("snapshot", "stream_and_sync");
            let metadata = serde_json::to_vec(&state).map_err(se)?;
            file.write_all(&hex::decode(blake3_digest(&metadata)).map_err(se)?)
                .map_err(se)?;
            write_frame(&mut file, &metadata)?;
            for id in Machine::chunk_ids(&state) {
                write_frame(
                    &mut file,
                    &machine.engine.chunk_store.read_chunk(&id).map_err(se)?,
                )?;
            }
            file.sync_all().map_err(se)?;
            file.seek(SeekFrom::Start(0)).map_err(se)?;
            Ok(Snapshot {
                meta,
                snapshot: Box::new(tokio::fs::File::from_std(file)),
            })
        })
        .await
        .map_err(se)?
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
        let _rpc = SlowPhase::new(if path == "snapshot" { "rpc_snapshot" } else { "rpc_other" }, "request_total");
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
            .timeout(std::time::Duration::from_secs(if path == "snapshot" {
                120
            } else {
                4
            }))
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
const APPEND_ENTRY_BYTES: usize = 10 * 1024 * 1024;
fn append_prefix(entries: &[Entry<Types>]) -> io::Result<usize> {
    struct Budget {
        left: usize,
        exceeded: bool,
    }
    impl Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.left {
                self.exceeded = true;
                return Err(io::Error::other("append byte budget"));
            }
            self.left -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut budget = Budget {
        left: APPEND_ENTRY_BYTES - 2,
        exceeded: false,
    };
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            if budget.left == 0 {
                return Ok(i);
            }
            budget.left -= 1;
        }
        if let Err(error) = serde_json::to_writer(&mut budget, e) {
            if budget.exceeded {
                return Ok(i.max(1));
            } // A valid single entry remains indivisible.
            return Err(err(error));
        }
    }
    Ok(entries.len())
}
impl RaftNetwork<Types> for Peer {
    async fn append_entries(
        &mut self,
        req: AppendEntriesRequest<Types>,
        _: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RpcErr> {
        let prefix =
            append_prefix(&req.entries).map_err(|e| RPCError::Network(NetworkError::new(&e)))?;
        if prefix < req.entries.len() {
            return Err(RPCError::PayloadTooLarge(
                PayloadTooLarge::new_entries_hint(prefix as u64),
            ));
        }
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
            let recovered = machine.restored_state().map_err(err)?;
            let mut s = machine.data.lock().await;
            *s = recovered;
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
            // Batch committed traffic instead of forcing one RPC per entry.
            max_payload_entries: 32,
            install_snapshot_timeout: 120_000,
            snapshot_max_chunk_size: 256 * 1024,
            election_timeout_min: 800,
            election_timeout_max: 1600,
            // Each snapshot streams all retained CAS references. Do not rebuild
            // it every 64 staged blocks; keep the lag threshold above cadence.
            snapshot_policy: openraft::SnapshotPolicy::LogsSinceLast(16_384),
            replication_lag_threshold: 32_768,
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
        if let Action::StageChunk { hex } = &c.action {
            if hex.len() > CHUNK_SIZE * 2 {
                return Err("oversized chunk payload".into());
            }
            let bytes = hex::decode(hex).map_err(|e| e.to_string())?;
            if bytes.is_empty() {
                return Err("empty staged chunk".into());
            }
        }
        if let Action::Put { hex, .. } = &c.action {
            if hex.len() > MAX_INLINE_BYTES * 2 || hex::decode(hex).is_err() {
                return Err("invalid or oversized file payload".into());
            }
        }
        let operation = if matches!(&c.action, Action::StageChunk { .. }) {
            "client_stage"
        } else {
            "client_other"
        };
        let correlation = slow_diagnostics().and_then(|d| d.command(&c));
        let mut wait = SlowPhase::new(operation, "raft_client_write_wait")
            .with_minimum(std::time::Duration::from_secs(1))
            .with_count(1).with_correlation(correlation);
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), self.raft.client_write(c)).await;
        wait.outcome = match &result { Ok(Ok(_)) => 1, Ok(Err(_)) => 2, Err(_) => 3 };
        result
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
        let epoch = match &action {
            Action::ReplayEpoch => 0,
            Action::Live(change) => change.epoch,
            _ => self
                .dispatch(Command {
                    id: format!("{id}-epoch"),
                    epoch: 0,
                    action: Action::ReplayEpoch,
                })
                .await?
                .get("epoch")
                .and_then(Value::as_u64)
                .ok_or_else(|| err("invalid replay epoch response"))?,
        };
        // The prepared command remains immutable across transport uncertainty.
        // A rollover is an explicit conflict, never an automatic fresh retry.
        self.dispatch(Command { id, epoch, action }).await
    }
    async fn dispatch(&self, command: Command) -> io::Result<Value> {
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
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.raft.ensure_linearizable(),
        )
        .await
        .map_err(|_| ReadError::unavailable("quorum read deadline exceeded"))?
        .map_err(ReadError::unavailable)?;
        if self
            .machine
            .engine
            .local_inode(&r.volume, &r.path)
            .await
            .map_err(ReadError::unavailable)?
            .size
            > MAX_INLINE_BYTES as u64
        {
            return Err(ReadError::unavailable(
                "use the streaming download endpoint for large files",
            ));
        }
        self.machine
            .engine
            .local_get_file(&r.volume, &r.path)
            .await
            .map(hex::encode)
            .map_err(|e| ReadError {
                missing: e.kind() == io::ErrorKind::NotFound,
                message: e.to_string(),
            })
    }
    pub async fn read(&self, volume: &str, path: &str) -> io::Result<Vec<u8>> {
        let (leader, node) = self.leader()?;
        let req = ReadRequest {
            volume: volume.into(),
            path: path.into(),
        };
        let data = if leader == self.config.node_id {
            self.local_read(req).await.map_err(ReadError::into_io)?
        } else {
            self.network
                .rpc::<_, String, ReadError>(leader, &node, "read", req)
                .await
                .map_err(|e| match e {
                    RPCError::RemoteError(remote) => remote.source.into_io(),
                    other => err(other),
                })?
        };
        hex::decode(data).map_err(err)
    }

    pub async fn local_inode(&self, r: ReadRequest) -> Result<Inode, ReadError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.raft.ensure_linearizable(),
        )
        .await
        .map_err(|_| ReadError::unavailable("quorum read deadline exceeded"))?
        .map_err(ReadError::unavailable)?;
        self.machine
            .engine
            .local_inode(&r.volume, &r.path)
            .await
            .map_err(|e| ReadError {
                missing: e.kind() == io::ErrorKind::NotFound,
                message: e.to_string(),
            })
    }
    pub async fn inode(&self, volume: &str, path: &str) -> io::Result<Inode> {
        let (leader, node) = self.leader()?;
        let req = ReadRequest {
            volume: volume.into(),
            path: path.into(),
        };
        if leader == self.config.node_id {
            self.local_inode(req).await.map_err(ReadError::into_io)
        } else {
            self.network
                .rpc::<_, Inode, ReadError>(leader, &node, "inode", req)
                .await
                .map_err(|e| match e {
                    RPCError::RemoteError(r) => r.source.into_io(),
                    other => err(other),
                })
        }
    }
    pub fn local_chunk(&self, id: &str) -> Result<String, String> {
        self.machine
            .engine
            .chunk_store
            .read_chunk(id)
            .map(hex::encode)
            .map_err(|e| e.to_string())
    }
    pub async fn chunk(&self, id: &str) -> io::Result<Vec<u8>> {
        if let Ok(data) = self.machine.engine.chunk_store.read_chunk(id) {
            return Ok(data);
        }
        for (node, url) in &self.config.peers {
            if *node == self.config.node_id {
                continue;
            }
            if let Ok(data) = self
                .network
                .rpc::<_, String, StringError>(*node, &BasicNode::new(url.clone()), "chunk", id)
                .await
            {
                let bytes = hex::decode(data).map_err(err)?;
                if self.machine.engine.chunk_store.write_chunk(&bytes)? != id {
                    return Err(err("peer chunk hash mismatch"));
                }
                return Ok(bytes);
            }
        }
        Err(err(
            "referenced chunk is unavailable from configured replicas",
        ))
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
    fn epoch_command(s: &MachineData, id: impl Into<String>, action: Action) -> Command {
        Command {
            id: id.into(),
            epoch: s.replay_epoch,
            action,
        }
    }
    fn empty_live(s: &MachineData, id: &str, remove: bool) -> Command {
        let expected = s
            .volumes
            .get("v")
            .and_then(|v| v.files.get("f"))
            .map(crate::live::version);
        let inode = (!remove).then(|| Inode {
            revision: 0,
            path: "f".into(),
            size: 0,
            digest: crate::live::digest(0, &[]),
            chunks_count: 0,
            chunks: vec![],
            updated_at: id.into(),
        });
        epoch_command(
            s,
            id,
            Action::Live(crate::live::Commit {
                id: id.into(),
                epoch: s.replay_epoch,
                hash: id.into(),
                volume: "v".into(),
                path: "f".into(),
                expected,
                inode,
                rename: None,
                destination: None,
                probe: false,
            }),
        )
    }
    #[tokio::test]
    async fn immutable_staging_does_not_expire_its_pending_live_commit() {
        let (_guard, _, machine) = Builder.build().await.unwrap();
        let mut s = MachineData::default();
        let commit = empty_live(&s, "pending-live", false);
        let aged = machine
            .engine
            .chunk_store
            .write_chunk(b"aged-staging")
            .unwrap();
        s.blobs.insert(aged.clone(), 12);
        for i in 0..600 {
            let command = Command {
                id: format!("stage-{i}"),
                epoch: 0,
                action: Action::StageChunk {
                    hex: hex::encode(b"z"),
                },
            };
            assert!(machine.execute(&mut s, &command).unwrap().is_string());
        }
        assert_eq!(s.replay_epoch, 0);
        assert_eq!(s.replay_admissions, 0);
        assert_eq!(s.staging_admissions, 88);
        assert!(s.seen.is_empty());
        assert!(s.seen_epochs.is_empty());
        assert!(s.live_ops.is_empty());
        assert_eq!(s.recent_staging.len(), 1);
        assert!(!s.blobs.contains_key(&aged));
        assert_eq!(
            machine.execute(&mut s, &commit).unwrap(),
            json!({"ok":true})
        );
        assert!(s.volumes["v"].files.contains_key("f"));
        assert_eq!(s.replay_admissions, 1);
    }
    #[tokio::test]
    async fn expired_create_cannot_resurrect_and_recent_retry_is_stable() {
        let (_guard, _, machine) = Builder.build().await.unwrap();
        let mut s = MachineData::default();
        let create = empty_live(&s, "original-create", false);
        machine.execute(&mut s, &create).unwrap();
        let remove = empty_live(&s, "original-delete", true);
        machine.execute(&mut s, &remove).unwrap();
        for i in 0..600 {
            let command = empty_live(&s, &format!("cycle-{i}"), i % 2 == 1);
            machine.execute(&mut s, &command).unwrap();
            assert!(s.seen.len() <= 512);
            assert!(s.live_ops.len() <= 512);
            assert_eq!(s.seen.len(), s.seen_epochs.len());
            assert_eq!(s.live_ops.len(), s.live_op_epochs.len());
        }
        assert_eq!(s.replay_epoch, 2);
        assert!(!s.volumes["v"].files.contains_key("f"));
        assert_eq!(
            machine.execute(&mut s, &create).unwrap()["error"],
            "live replay epoch expired"
        );
        assert!(!s.volumes["v"].files.contains_key("f"));
        let recent = empty_live(&s, "recent-create", false);
        machine.execute(&mut s, &recent).unwrap();
        let revision = s.live_revision;
        let admissions = s.replay_admissions;
        machine.execute(&mut s, &recent).unwrap();
        assert_eq!(s.live_revision, revision);
        assert_eq!(s.replay_admissions, admissions);
        let query = Command {
            id: recent.id.clone(),
            epoch: u64::MAX,
            action: Action::ReplayEpoch,
        };
        assert_eq!(machine.execute(&mut s, &query).unwrap()["epoch"], 2);
        assert_eq!(s.replay_admissions, admissions);
        let mut mismatched = recent;
        mismatched.id = "different-command".into();
        if let Action::Live(change) = &mut mismatched.action {
            change.epoch = 1;
        }
        assert_eq!(
            machine.execute(&mut s, &mismatched).unwrap()["error"],
            "live replay epoch expired"
        );
    }
    #[tokio::test]
    async fn replay_epoch_and_previous_generation_survive_durable_snapshot() {
        let (_guard, db, mut machine) = Builder.build().await.unwrap();
        let mut commands = Vec::new();
        for i in 0..300u64 {
            commands.push(Entry {
                log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), i + 1),
                payload: EntryPayload::Normal(Command {
                    id: format!("admission-{i}"),
                    epoch: i / 256,
                    action: Action::DeleteVolume {
                        name: "absent".into(),
                    },
                }),
            });
        }
        machine.apply(commands).await.unwrap();
        let persisted = machine.restored_state().unwrap();
        assert_eq!(persisted.replay_epoch, 1);
        assert_eq!(persisted.replay_admissions, 44);
        let (_other, _, mut target) = Builder.build().await.unwrap();
        let image = machine.build_snapshot().await.unwrap();
        target
            .install_snapshot(&image.meta, image.snapshot)
            .await
            .unwrap();
        let mut s = target.data.lock().await;
        assert_eq!(s.replay_epoch, 1);
        let before = s.replay_admissions;
        let previous = Command {
            id: "admission-255".into(),
            epoch: 0,
            action: Action::DeleteVolume {
                name: "absent".into(),
            },
        };
        assert_eq!(target.execute(&mut s, &previous).unwrap(), json!(false));
        assert_eq!(s.replay_admissions, before);
        // The global file revision never resets when history rotates.
        s.live_revision = 1000;
        let command = empty_live(&s, "post-restore", false);
        target.execute(&mut s, &command).unwrap();
        assert_eq!(s.live_revision, 1001);
    }
    #[tokio::test]
    async fn blob_pruning_keeps_current_snapshot_and_pending_references() {
        let (_guard, _, machine) = Builder.build().await.unwrap();
        let mut s = MachineData::default();
        for (id, action) in [
            (
                "first",
                Action::Put {
                    volume: "snap".into(),
                    path: "f".into(),
                    hex: hex::encode(b"snapshot"),
                    at: "t".into(),
                },
            ),
            (
                "snapshot",
                Action::Snapshot {
                    volume: "snap".into(),
                    name: "kept".into(),
                    id: "snapshot-id".into(),
                    at: "t".into(),
                },
            ),
            (
                "remove",
                Action::DeleteVolume {
                    name: "snap".into(),
                },
            ),
            (
                "current",
                Action::Put {
                    volume: "current".into(),
                    path: "f".into(),
                    hex: hex::encode(b"current"),
                    at: "t".into(),
                },
            ),
            (
                "pending",
                Action::StageChunk {
                    hex: hex::encode(b"pending"),
                },
            ),
        ] {
            let command = epoch_command(&s, id, action);
            machine.execute(&mut s, &command).unwrap();
        }
        let pending = s.recent_staging.back().unwrap().clone();
        // Simulate an aged unreferenced block without writing 1 TiB of payload.
        let aged = machine.engine.chunk_store.write_chunk(b"aged").unwrap();
        s.blobs.insert(aged.clone(), 4);
        for i in 0..256 {
            let command = epoch_command(
                &s,
                format!("advance-{i}"),
                Action::DeleteVolume {
                    name: "absent".into(),
                },
            );
            machine.execute(&mut s, &command).unwrap();
        }
        assert!(!s.blobs.contains_key(&aged));
        assert!(s.blobs.contains_key(&pending));
        for inode in s
            .volumes
            .values()
            .flat_map(|v| v.files.values())
            .chain(s.snapshots.iter().flat_map(|v| v.files_tree.values()))
        {
            assert!(validate_layout(inode, &s.blobs).is_ok());
            for c in &inode.chunks {
                machine.engine.chunk_store.read_chunk(&c.id).unwrap();
            }
        }
        let invalid = Inode {
            revision: 0,
            path: "aged".into(),
            size: 4,
            digest: format!("blake3:{}", blake3_digest(b"aged")),
            chunks_count: 1,
            chunks: vec![ChunkRef {
                id: aged,
                size: 4,
                offset: 0,
            }],
            updated_at: "t".into(),
        };
        let command = epoch_command(
            &s,
            "expired-staging",
            Action::CommitFile {
                volume: "current".into(),
                inode: invalid,
            },
        );
        assert!(machine
            .execute(&mut s, &command)
            .unwrap()
            .get("error")
            .is_some());
        assert!(!s.volumes["current"].files.contains_key("aged"));
    }
    #[tokio::test]
    async fn batched_apply_is_durable_before_return_and_retains_snapshot_chunks() {
        let (_guard, _, mut machine) = Builder.build().await.unwrap();
        let mut actions = vec![
            Action::Put {
                volume: "v".into(),
                path: "f".into(),
                hex: hex::encode(b"before"),
                at: "t".into(),
            },
            Action::Snapshot {
                volume: "v".into(),
                name: "kept".into(),
                id: "kept-id".into(),
                at: "t".into(),
            },
        ];
        for i in 0..254 {
            actions.push(Action::Put {
                volume: "v".into(),
                path: "f".into(),
                hex: hex::encode(format!("after-{i}")),
                at: "t".into(),
            });
        }
        let entries = actions
            .into_iter()
            .enumerate()
            .map(|(i, action)| Entry {
                log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), i as u64 + 1),
                payload: EntryPayload::Normal(Command {
                    id: format!("batch-{i}"),
                    epoch: 0,
                    action,
                }),
            })
            .collect::<Vec<_>>();
        let results = machine.apply(entries).await.unwrap();
        assert_eq!(results.len(), 256);
        assert_eq!(results[1]["name"], "kept");
        assert_eq!(
            results[255]["digest"],
            format!("blake3:{}", blake3_digest(b"after-253"))
        );
        // A fresh SQLite connection must observe the full final applied batch,
        // including replay rotation and the chunks referenced by the snapshot.
        let reopened = Db::open(&_guard.0.join("raft.sqlite")).unwrap();
        let reader = Machine {
            db: reopened,
            engine: machine.engine.clone(),
            data: AsyncMutex::new(MachineData::default()),
        };
        let persisted = reader.restored_state().unwrap();
        assert_eq!(persisted.applied.unwrap().index, 256);
        assert_eq!(persisted.replay_epoch, 1);
        assert_eq!(persisted.replay_admissions, 0);
        assert_eq!(persisted.blobs.len(), 2);
        assert_eq!(persisted.snapshots.len(), 1);
        assert_eq!(
            machine.engine.local_get_file("v", "f").await.unwrap(),
            b"after-253"
        );
        let (_other, _, mut target) = Builder.build().await.unwrap();
        let image = machine.build_snapshot().await.unwrap();
        target
            .install_snapshot(&image.meta, image.snapshot)
            .await
            .unwrap();
        assert_eq!(
            target.engine.local_get_file("v", "f").await.unwrap(),
            b"after-253"
        );
        let restored = target.data.lock().await;
        let inode = &restored.snapshots[0].files_tree["f"];
        assert_eq!(
            target
                .engine
                .chunk_store
                .read_chunk(&inode.chunks[0].id)
                .unwrap(),
            b"before"
        );
        assert_eq!(restored.replay_epoch, persisted.replay_epoch);
    }
    #[tokio::test]
    async fn incremental_journal_migrates_checkpoint_and_preserves_newer_tail() {
        let (_guard, db, mut machine) = Builder.build().await.unwrap();
        let stage = |index, bytes: &[u8]| Entry {
            log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), index),
            payload: EntryPayload::Normal(Command {
                id: format!("stage-{index}"),
                epoch: 0,
                action: Action::StageChunk {
                    hex: hex::encode(bytes),
                },
            }),
        };
        // An existing full-machine record remains a valid migration base.
        db.set("machine", &MachineData::default()).unwrap();
        machine.apply([stage(1, b"first")]).await.unwrap();
        let first = machine.data.lock().await.clone();
        machine.apply([stage(2, b"second")]).await.unwrap();
        db.checkpoint(&first, false).unwrap();
        let restored = machine.restored_state().unwrap();
        assert_eq!(restored.applied.unwrap().index, 2);
        assert_eq!(restored.blobs.len(), 2);
        assert_eq!(
            db.get::<Option<LogId<u64>>>("machine_base")
                .unwrap()
                .flatten(),
            first.applied
        );
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM machine_deltas", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            1
        );
        db.checkpoint(&restored, false).unwrap();
        db.checkpoint(&first, false).unwrap(); // Late older snapshot cannot regress base.
        assert_eq!(machine.restored_state().unwrap().applied.unwrap().index, 2);
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM machine_deltas", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        let image = machine.build_snapshot().await.unwrap();
        let (_other, target_db, mut target) = Builder.build().await.unwrap();
        target.apply([stage(1, b"unrelated")]).await.unwrap();
        target
            .install_snapshot(&image.meta, image.snapshot)
            .await
            .unwrap();
        assert_eq!(target.restored_state().unwrap().blobs.len(), 2);
        assert_eq!(
            target_db
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM machine_deltas", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn sqlite_log_reader_filters_bounds_before_deserializing() {
        let (_guard, mut db, _) = Builder.build().await.unwrap();
        {
            let c = db.conn.lock().unwrap();
            c.execute("INSERT INTO logs(idx,entry) VALUES(0,'not JSON')", [])
                .unwrap();
            for i in 1..=5u64 {
                let entry = Entry::<Types> {
                    log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), i),
                    payload: EntryPayload::Blank,
                };
                c.execute(
                    "INSERT INTO logs(idx,entry) VALUES(?,?)",
                    params![i, serde_json::to_string(&entry).unwrap()],
                )
                .unwrap();
            }
        }
        let found = db.try_get_log_entries(2..5).await.unwrap();
        assert_eq!(
            found.iter().map(|v| v.log_id.index).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(db.try_get_log_entries(5..=5).await.unwrap().len(), 1);
        assert!(db.try_get_log_entries(3..3).await.unwrap().is_empty());
        assert!(db
            .try_get_log_entries((Bound::Excluded(u64::MAX), Bound::Unbounded))
            .await
            .unwrap()
            .is_empty());
    }
    #[test]
    fn append_byte_budget_keeps_one_valid_large_entry() {
        let large = Entry::<Types> {
            log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), 1),
            payload: EntryPayload::Normal(Command {
                id: "large".into(),
                epoch: 0,
                action: Action::Put {
                    volume: "v".into(),
                    path: "f".into(),
                    hex: "00".repeat(MAX_INLINE_BYTES),
                    at: "t".into(),
                },
            }),
        };
        assert_eq!(append_prefix(std::slice::from_ref(&large)).unwrap(), 1);
        let blank = Entry::<Types> {
            log_id: LogId::new(openraft::CommittedLeaderId::new(1, 1), 2),
            payload: EntryPayload::Blank,
        };
        assert_eq!(append_prefix(&[large, blank]).unwrap(), 1);
    }
    fn correlation_test_sink(key: Option<[u8; 32]>, capacity: usize)
        -> (&'static SlowDiagnostics, std::sync::mpsc::Receiver<SlowEvent>)
    {
        use std::sync::atomic::AtomicU64;
        let (sender, receiver) = std::sync::mpsc::sync_channel(capacity);
        (Box::leak(Box::new(SlowDiagnostics {
            sender, start: std::time::Instant::now(),
            threshold: std::time::Duration::from_secs(60),
            attempted: Arc::new(AtomicU64::new(0)), dropped: Arc::new(AtomicU64::new(0)),
            capped: Arc::new(AtomicU64::new(0)), next_span: AtomicU64::new(1), correlation_key: key,
        })), receiver)
    }
    fn correlation_test_phase(d: &'static SlowDiagnostics, ctx: Option<Correlation>) -> SlowPhase {
        SlowPhase {
            diagnostic: Some(d), operation: "test", phase: "join",
            start: Some(std::time::Instant::now()), minimum: std::time::Duration::from_secs(60),
            count: 0, correlation: None, span: 0,
            start_stamp: DiagnosticStamp::default(), outcome: 0,
        }.with_correlation(ctx)
    }
    #[test]
    fn correlation_opaque_identity_joins_client_entry_and_batch_without_wire_changes() {
        let command = Command { id: "private-client-id".into(), epoch: 7,
            action: Action::StageChunk { hex: "736563726574".into() } };
        let before = serde_json::to_value(&command).unwrap();
        let (d, rx) = correlation_test_sink(Some([41; 32]), 256);
        let entries = vec![
            Entry::<Types> { log_id: LogId::new(openraft::CommittedLeaderId::new(2, 3), 11), payload: EntryPayload::Normal(command.clone()) },
            Entry::<Types> { log_id: LogId::new(openraft::CommittedLeaderId::new(4, 5), 12), payload: EntryPayload::Blank },
        ];
        let batch = d.batch(&entries).unwrap();
        let entry = d.entry(batch, &entries[0]);
        assert_eq!(d.command(&command).unwrap().command, entry.command);
        assert_eq!(entry.batch, batch.batch);
        assert_eq!(entry.first, [2, 3, 11]);
        assert_eq!(entry.last, [4, 5, 12]);
        assert_eq!(entry.entry, Some([2, 3, 11]));
        assert!(d.entry(batch, &entries[1]).command.is_none());
        let mut other = command.clone(); other.epoch += 1;
        assert_ne!(entry.command, d.command(&other).unwrap().command);
        assert_ne!(opaque_command(&[42; 32], &command), entry.command);
        other.id = "x".repeat(257);
        assert!(d.command(&other).is_none());
        drop(correlation_test_phase(d, Some(entry)));
        let event = rx.try_recv().unwrap();
        let mut bytes = Vec::new();
        write_slow_event(&mut bytes, &event, [1, 0, 0]).unwrap();
        let line = String::from_utf8(bytes).unwrap();
        assert!(line.len() < 1600);
        assert!(!line.contains("private-client-id") && !line.contains("736563726574"));
        assert!(line.contains("first_index=11") && line.contains("last_index=12") && line.contains("entry_index=11"));
        assert_eq!(serde_json::to_value(&command).unwrap(), before);
        assert_eq!(before, json!({"id":"private-client-id","epoch":7,"action":{"StageChunk":{"hex":"736563726574"}}}));
        let (off, off_rx) = correlation_test_sink(None, 256);
        assert!(off.command(&command).is_none() && off.batch(&entries).is_none());
        drop(correlation_test_phase(off, None));
        assert!(off_rx.try_recv().is_err());
    }
    #[test]
    fn correlation_fast_join_events_obey_shared_queue_drop_and_cap() {
        use std::sync::atomic::Ordering::Relaxed;
        let (d, rx) = correlation_test_sink(Some([1; 32]), 256);
        for _ in 0..10_010 {
            // No sleep: even subthreshold joins must be observable or counted.
            drop(correlation_test_phase(d, Some(Correlation::default())));
        }
        assert_eq!(d.attempted.load(Relaxed), 10_000);
        assert_eq!(d.dropped.load(Relaxed), 10_000 - 256);
        assert_eq!(d.capped.load(Relaxed), 10);
        assert_eq!(rx.try_iter().count(), 256);
        drop(correlation_test_phase(d, Some(Correlation::default())));
        assert_eq!(d.capped.load(Relaxed), 11);
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn correlation_context_does_not_change_persisted_entries_or_marker() {
        let (_g1, db1, _) = Builder.build().await.unwrap();
        let (_g2, db2, _) = Builder.build().await.unwrap();
        let entry = Entry::<Types> {
            log_id: LogId::new(openraft::CommittedLeaderId::new(2, 3), 11),
            payload: EntryPayload::Normal(Command { id: "persist-wire".into(), epoch: 7,
                action: Action::StageChunk { hex: "00".into() } }),
        };
        let entries = [entry];
        let (d, _rx) = correlation_test_sink(Some([9; 32]), 256);
        db1.persist_apply(&entries, Some(entries[0].log_id), None).unwrap();
        db2.persist_apply(&entries, Some(entries[0].log_id), d.batch(&entries)).unwrap();
        for db in [&db1, &db2] {
            let c = db.conn.lock().unwrap();
            let raw: String = c.query_row("SELECT entries FROM machine_deltas WHERE last_idx=11", [], |r| r.get(0)).unwrap();
            assert_eq!(raw, serde_json::to_string(&entries).unwrap());
            let marker: String = c.query_row("SELECT v FROM meta WHERE k='machine_applied'", [], |r| r.get(0)).unwrap();
            assert_eq!(marker, serde_json::to_string(&Some(entries[0].log_id)).unwrap());
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn correlation_blocking_worker_clock_and_tid_are_real_endpoint_observations() {
        let caller = diagnostic_stamp();
        let (d, rx) = correlation_test_sink(Some([2; 32]), 256);
        // Creation before handoff and destruction on the worker must retain BOTH
        // TIDs; no span claims one async task ran on a single OS thread.
        let queued = correlation_test_phase(d, Some(Correlation::default()));
        let worker = std::thread::spawn(move || {
            let here = diagnostic_stamp();
            drop(queued);
            let phase = correlation_test_phase(d, Some(Correlation::default()));
            drop(phase);
            here
        }).join().unwrap();
        let queued_event = rx.try_recv().unwrap();
        let work_event = rx.try_recv().unwrap();
        assert!(caller.monotonic_ns > 0 && worker.monotonic_ns >= caller.monotonic_ns);
        assert!(caller.tid > 0 && worker.tid > 0 && caller.tid != worker.tid);
        assert_eq!(queued_event.start_stamp.tid, caller.tid);
        assert_eq!(queued_event.end_stamp.tid, worker.tid);
        assert_eq!(work_event.start_stamp.tid, worker.tid);
        assert_eq!(work_event.end_stamp.tid, worker.tid);
        assert!(work_event.end_stamp.monotonic_ns >= work_event.start_stamp.monotonic_ns);
        // Outcome zero explicitly does not certify a committed operation.
        assert_eq!(queued_event.outcome, 0);
    }
    #[test]
    fn slow_client_minimum_preserves_budget_and_emits_count() {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        use std::time::{Duration, Instant};
        let (sender, receiver) = std::sync::mpsc::sync_channel(256);
        let now = Instant::now();
        let diagnostic = Box::leak(Box::new(SlowDiagnostics {
            sender, start: now - Duration::from_secs(10),
            threshold: Duration::from_millis(50),
            attempted: Arc::new(AtomicU64::new(0)), dropped: Arc::new(AtomicU64::new(0)),
            capped: Arc::new(AtomicU64::new(0)), next_span: AtomicU64::new(1), correlation_key: None,
        }));
        let mut normal = SlowPhase::new("client_stage", "raft_client_write_wait")
            .with_minimum(Duration::from_secs(1)).with_count(1);
        normal.diagnostic = Some(diagnostic);
        normal.start = Some(Instant::now());
        drop(normal);
        assert_eq!(diagnostic.attempted.load(Relaxed), 0);
        assert!(receiver.try_recv().is_err());
        let mut slow = SlowPhase::new("client_stage", "raft_client_write_wait")
            .with_minimum(Duration::from_secs(1)).with_count(7);
        slow.diagnostic = Some(diagnostic);
        slow.start = Some(now - Duration::from_secs(2));
        drop(slow);
        let event = receiver.try_recv().unwrap();
        assert_eq!(event.count, 7);
        assert!(event.elapsed_us >= 2_000_000);
        assert_eq!(diagnostic.attempted.load(Relaxed), 1);
    }
    #[test]
    fn slow_phase_queue_and_total_emission_are_bounded() {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        let (sender, receiver) = std::sync::mpsc::sync_channel(256);
        let diagnostic = Box::leak(Box::new(SlowDiagnostics {
            sender,
            start: std::time::Instant::now(),
            threshold: std::time::Duration::ZERO,
            attempted: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
            capped: Arc::new(AtomicU64::new(0)), next_span: AtomicU64::new(1), correlation_key: None,
        }));
        for _ in 0..10_010 {
            drop(SlowPhase {
                diagnostic: Some(diagnostic), operation: "test", phase: "bounded",
                start: Some(std::time::Instant::now()),
                minimum: std::time::Duration::ZERO, count: 0, correlation: None, span: 0, start_stamp: DiagnosticStamp::default(), outcome: 0,
            });
        }
        assert_eq!(diagnostic.attempted.load(Relaxed), 10_000);
        assert_eq!(diagnostic.dropped.load(Relaxed), 10_000 - 256);
        assert_eq!(receiver.try_iter().count(), 256);
        // A drained queue still cannot emit after the process-wide budget.
        drop(SlowPhase {
            diagnostic: Some(diagnostic), operation: "test", phase: "bounded",
            start: Some(std::time::Instant::now()),
            minimum: std::time::Duration::ZERO, count: 0, correlation: None, span: 0, start_stamp: DiagnosticStamp::default(), outcome: 0,
        });
        assert!(receiver.try_recv().is_err());
        assert_eq!(diagnostic.capped.load(Relaxed), 11);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn raft_storage_mutex_wait_keeps_single_thread_timer_responsive() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;

        let (_guard, mut db, _) = Builder.build().await.unwrap();
        let held = db.clone();
        let released = Arc::new(AtomicBool::new(false));
        let released_by_holder = released.clone();
        let (ready, locked) = tokio::sync::oneshot::channel();
        let holder = std::thread::spawn(move || {
            let connection = held.conn.lock().unwrap();
            ready.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(250));
            released_by_holder.store(true, Ordering::SeqCst);
            drop(connection);
        });
        locked.await.unwrap();
        // Poll storage first: the old synchronous lock blocks this sole runtime
        // thread until release, so the timer's assertion would fail.
        let (vote, ()) = tokio::join!(biased; db.read_vote(), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(
                !released.load(Ordering::SeqCst),
                "unrelated timer did not progress while storage waited for SQLite"
            );
        });
        assert!(vote.unwrap().is_none());
        holder.join().unwrap();
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
            epoch: 0,
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
        let persisted = sm.restored_state().unwrap();
        let reopened = Db::open(&_guard.0.join("raft.sqlite")).unwrap();
        assert_eq!(
            Machine {
                db: reopened,
                engine: sm.engine.clone(),
                data: AsyncMutex::new(MachineData::default())
            }
            .restored_state()
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
            epoch: command.epoch,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadError {
    pub missing: bool,
    pub message: String,
}
impl ReadError {
    fn unavailable(e: impl std::fmt::Display) -> Self {
        Self {
            missing: false,
            message: e.to_string(),
        }
    }
    fn into_io(self) -> io::Error {
        io::Error::new(
            if self.missing {
                io::ErrorKind::NotFound
            } else {
                io::ErrorKind::Other
            },
            self.message,
        )
    }
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ReadError {}

fn write_frame(w: &mut impl Write, bytes: &[u8]) -> SResult<()> {
    w.write_all(&(bytes.len() as u64).to_be_bytes())
        .map_err(se)?;
    w.write_all(bytes).map_err(se)
}
fn read_frame(r: &mut impl Read, max: usize) -> SResult<Vec<u8>> {
    let mut len = [0; 8];
    r.read_exact(&mut len).map_err(se)?;
    let len = u64::from_be_bytes(len);
    if len > max as u64 {
        return Err(se("snapshot frame exceeds its bound"));
    }
    let mut bytes = vec![0; len as usize];
    r.read_exact(&mut bytes).map_err(se)?;
    Ok(bytes)
}

/// Payload hashes are calculated while ingesting and verified while reading.
/// Committing an inode checks references to already committed immutable blocks;
/// it never rescans an entire VM disk while holding the state-machine lock.
fn validate_layout(inode: &Inode, sizes: &BTreeMap<String, usize>) -> SResult<()> {
    if inode.chunks_count != inode.chunks.len() {
        return Err(se("chunk count mismatch"));
    }
    if inode.digest.starts_with("merkle-blake3:") {
        return crate::live::validate(inode, sizes).map_err(se);
    }
    let digest = inode
        .digest
        .strip_prefix("blake3:")
        .ok_or_else(|| se("invalid file digest"))?;

    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(se("invalid file digest"));
    }
    let mut offset = 0u64;
    for c in &inode.chunks {
        if c.offset != offset
            || c.size == 0
            || c.size > CHUNK_SIZE
            || sizes.get(&c.id) != Some(&c.size)
        {
            return Err(se("uncommitted or invalid chunk reference"));
        }
        offset = offset
            .checked_add(c.size as u64)
            .ok_or_else(|| se("file size overflow"))?;
    }
    if offset != inode.size {
        return Err(se("file size mismatch"));
    }
    Ok(())
}
