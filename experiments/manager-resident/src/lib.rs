//! Bounded resident replication laboratory. No executable control authority.
use fs2::FileExt;
use podmesh_manager_ha_lab::durable::{
    DurableError, RefusalReason, Request, Response, Store, DEFAULT_FULL_VERIFICATION_INTERVAL,
};
use podmesh_manager_network_lab::{
    ConfigurationFile, ErrorSource, ServedDecision, ServedImport, ServedRefusal,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::TcpListener,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
enum AppendCompletion {
    Observed(Vec<u8>),
    Refused,
    Uncertain,
}
type AppendJob = (mpsc::Receiver<AppendCompletion>, thread::JoinHandle<()>);

enum AppendStartError {
    Refused,
    Busy,
    CatchingUp,
}

pub mod cli;

/// Default delay after which a snapshot that a peer already acknowledged is
/// pushed to that peer again although it has not changed. An idle replica adds
/// its twelve audit rows once per refresh, about 1,700 a day at three replicas,
/// and an idle link confirms its peer once per refresh.
pub const DEFAULT_UNCHANGED_SNAPSHOT_REFRESH: Duration = Duration::from_secs(600);

/// Default catch-up window: how long after it starts exchanging a process whose
/// store held facts of its own origin waits for peers it has not caught up with
/// before it appends local facts anyway.
pub const DEFAULT_CATCH_UP_WINDOW: Duration = Duration::from_secs(15);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub network: ConfigurationFile,
    pub control_socket: PathBuf,
    pub observation_writer_uid: u32,
    pub interval_ms: u64,
    pub max_backoff_ms: u64,
    pub incoming_workers: usize,
    /// Interval of the background complete store verification; absent means
    /// `DEFAULT_FULL_VERIFICATION_INTERVAL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_verification_interval_ms: Option<u64>,
    /// Delay before re-pushing a snapshot the peer already acknowledged; absent
    /// means `DEFAULT_UNCHANGED_SNAPSHOT_REFRESH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchanged_snapshot_refresh_ms: Option<u64>,
    /// Catch-up window of a store that held facts of this replica's own origin
    /// at start; absent means `DEFAULT_CATCH_UP_WINDOW`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up_window_ms: Option<u64>,
}

impl Configuration {
    pub fn validate(&self) -> Result<()> {
        self.validate_inspection()?;
        self.network.validate()?;
        if !(100..=60_000).contains(&self.interval_ms)
            || !(self.interval_ms..=300_000).contains(&self.max_backoff_ms)
            || !(1..=8).contains(&self.incoming_workers)
            || self.network.peers.len() > 15
        {
            return Err("invalid interval, backoff, workers or peer count".into());
        }
        if self
            .full_verification_interval_ms
            .is_some_and(|interval| !(1_000..=86_400_000).contains(&interval))
            || self
                .unchanged_snapshot_refresh_ms
                .is_some_and(|refresh| !(self.interval_ms..=3_600_000).contains(&refresh))
        {
            return Err("invalid full verification interval or unchanged snapshot refresh".into());
        }
        // The universe entrypoint gives a start 25 seconds, the store's first
        // open included, so the window stays well below that budget.
        if self
            .catch_up_window_ms
            .is_some_and(|window| !(1_000..=20_000).contains(&window))
        {
            return Err("invalid catch-up window".into());
        }
        if !self.network.database_path.is_absolute() {
            return Err("database requires a bounded absolute local path".into());
        }
        if !self.control_socket.is_absolute() || self.control_socket.as_os_str().len() > 100 {
            return Err("control socket requires a bounded absolute local path".into());
        }
        if self.network.bind.port() == 0
            || self
                .network
                .peers
                .iter()
                .any(|peer| peer.endpoint.port() == 0)
        {
            return Err("static endpoints require nonzero ports".into());
        }
        let parent = self.control_socket.parent().ok_or("socket has no parent")?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err("control socket parent must be a private directory".into());
        }
        Ok(())
    }

    /// Interval between two background complete verifications of the store.
    #[must_use]
    pub fn full_verification_interval(&self) -> Duration {
        self.full_verification_interval_ms
            .map_or(DEFAULT_FULL_VERIFICATION_INTERVAL, Duration::from_millis)
    }

    /// Delay after which an acknowledged, unchanged snapshot is pushed again.
    #[must_use]
    pub fn unchanged_snapshot_refresh(&self) -> Duration {
        self.unchanged_snapshot_refresh_ms
            .map_or(DEFAULT_UNCHANGED_SNAPSHOT_REFRESH, Duration::from_millis)
    }

    /// Catch-up window of a store that held facts of its own origin at start.
    #[must_use]
    pub fn catch_up_window(&self) -> Duration {
        self.catch_up_window_ms
            .map_or(DEFAULT_CATCH_UP_WINDOW, Duration::from_millis)
    }

    pub(crate) fn validate_inspection(&self) -> Result<()> {
        validate_control_token(&self.network.manager.logical_manager_id)?;
        for replica in &self.network.manager.replicas {
            validate_control_token(&replica.replica_id)?;
            validate_control_token(&replica.host_id)?;
        }
        for grant in &self.network.manager.grants {
            validate_control_scope(&grant.scope)?;
            validate_control_token(&grant.owner_replica_id)?;
        }
        let topology = self.network.manager.topology()?;
        topology.instantiate(&self.network.replica_id)?;
        Ok(())
    }
}

#[derive(Clone, Serialize, Default)]
pub struct PeerStatus {
    pub authenticated_successes: u64,
    pub failures: u64,
    pub last_success_age_ms: Option<u64>,
    /// Age of the end of the last exchange attempt with this peer, whatever its
    /// outcome; equal to `last_success_age_ms` when that attempt succeeded.
    pub last_attempt_age_ms: Option<u64>,
    pub acknowledged_history_len: Option<usize>,
    pub local_history_len_at_attempt: Option<usize>,
    pub history_count_delta: Option<i128>,
    pub outcome: String,
    pub next_attempt_in_ms: u64,
    /// The peer's authenticated receipt acknowledges the local snapshot, which
    /// has not changed since, the refresh has not elapsed and no push is due.
    pub acknowledged_unchanged: bool,
    /// Effective delay before an acknowledged, unchanged snapshot is pushed again.
    pub refresh_ms: u64,
    /// Longest delay between two attempts after failures.
    pub max_backoff_ms: u64,
    /// Attempts to this peer that served a push-back: an import from it lacked
    /// facts this replica holds, so its acknowledgement was forgotten and the
    /// attempt made due at once.
    pub push_backs: u64,
    /// Pushes to this peer that it refused with a signed reason.
    pub authenticated_refusals: u64,
    /// The closed reason of the last of them.
    pub last_refusal_reason: Option<RefusalReason>,
    /// Authenticated imports from this peer that this replica refused.
    pub refused_imports: u64,
    /// Event identity collisions between this peer's history and this replica's.
    pub identity_collisions: IdentityCollisions,
}

/// Facts that this replica's history and one peer's hold under one event ID with
/// other bytes. Each side then refuses the other's imports for good, and the
/// signed refusal names only `policy_violation`: the receiving side finds the
/// collision in the refused snapshot, and the sending side can tell that a
/// refusal is one only once it has found a collision in the peer's own pushes.
#[derive(Clone, Serialize, Default)]
pub struct IdentityCollisions {
    /// Imports from the peer refused while its snapshot held an event ID this
    /// replica holds with other bytes.
    pub imports_refused: u64,
    /// Pushes to the peer that it refused with `policy_violation` once such a
    /// collision was found: the peer still holds the fact it refused ours for.
    pub pushes_refused: u64,
    /// The smallest colliding event ID of the latest such import.
    pub event_id: Option<String>,
}

/// What caught this process up with its peers.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CaughtUpBy {
    /// The topology declares no peer.
    NoPeers,
    /// Every peer was imported from or matched.
    EveryPeer,
    /// The store held facts of this replica's own origin at start, and the
    /// catch-up window elapsed.
    Window,
}

#[derive(Serialize)]
struct CatchUpStatus {
    caught_up: bool,
    caught_up_by: Option<CaughtUpBy>,
    caught_up_after_ms: Option<u64>,
    peers_imported: Vec<String>,
    peers_matched: Vec<String>,
    peers_missing: Vec<String>,
    own_facts_at_start: usize,
    window_ms: u64,
}

#[derive(Serialize)]
struct Status {
    kind: &'static str,
    replica_id: String,
    canonical_inspection_available_via: &'static str,
    peers: BTreeMap<String, PeerStatus>,
    catch_up: CatchUpStatus,
    active_incoming: usize,
    peak_incoming: usize,
    rejected_connections: usize,
    append_worker_failures: usize,
    incoming_limit: usize,
    outgoing_limit: usize,
    activation_authority: bool,
}

/// Whether this process has caught up with its peers, which it must have before
/// its first local append. A store that lacks some facts of this replica's own
/// origin, deleted or restored from an older copy, would otherwise append under
/// producer sequences its peers already hold with other bytes, and both sides
/// would then refuse every import from the other. Importing those facts back
/// from the peers first makes the next local fact take the next sequence.
///
/// A peer is caught up with once this process has committed or replayed an
/// authenticated import from it, or once that peer's authenticated receipt for
/// a push of this process counted exactly the pushed facts, so that it held no
/// fact this replica lacked. The state latches: it never reverts in a process.
struct CatchUp {
    /// When this process started exchanging.
    started: Instant,
    window: Duration,
    own_facts_at_start: usize,
    peers: BTreeSet<String>,
    imported: BTreeSet<String>,
    matched: BTreeSet<String>,
    caught_up: Option<(CaughtUpBy, Instant)>,
}

impl CatchUp {
    fn new(peers: BTreeSet<String>, own_facts_at_start: usize, window: Duration) -> Self {
        let started = Instant::now();
        Self {
            caught_up: peers.is_empty().then_some((CaughtUpBy::NoPeers, started)),
            started,
            window,
            own_facts_at_start,
            peers,
            imported: BTreeSet::new(),
            matched: BTreeSet::new(),
        }
    }

    fn missing(&self) -> impl Iterator<Item = &String> {
        self.peers
            .iter()
            .filter(|peer| !self.imported.contains(*peer) && !self.matched.contains(*peer))
    }

    /// Whether this process may append, latching the first condition that held.
    /// Only a store that held facts of its own origin at start may append
    /// before every peer is caught up with, and only after the window: a store
    /// without any never appends before every peer, so it cannot fork its own
    /// history.
    fn evaluate(&mut self, now: Instant) -> bool {
        if self.caught_up.is_none() {
            let window_end = self.started + self.window;
            if self.own_facts_at_start > 0 && now >= window_end {
                self.caught_up = Some((CaughtUpBy::Window, window_end));
            } else if self.missing().next().is_none() {
                self.caught_up = Some((CaughtUpBy::EveryPeer, now));
            }
        }
        self.caught_up.is_some()
    }

    fn record_import(&mut self, peer: &str, now: Instant) {
        if self.peers.contains(peer) {
            self.imported.insert(peer.into());
        }
        self.evaluate(now);
    }

    fn record_match(&mut self, peer: &str, now: Instant) {
        if self.peers.contains(peer) {
            self.matched.insert(peer.into());
        }
        self.evaluate(now);
    }

    fn status(&mut self, now: Instant) -> CatchUpStatus {
        self.evaluate(now);
        CatchUpStatus {
            caught_up: self.caught_up.is_some(),
            caught_up_by: self.caught_up.map(|(by, _)| by),
            caught_up_after_ms: self
                .caught_up
                .map(|(_, at)| millis(at.saturating_duration_since(self.started))),
            peers_imported: self.imported.iter().cloned().collect(),
            peers_matched: self.matched.iter().cloned().collect(),
            peers_missing: self.missing().cloned().collect(),
            own_facts_at_start: self.own_facts_at_start,
            window_ms: millis(self.window),
        }
    }
}

/// A local snapshot digest that a peer acknowledged with an authenticated receipt.
struct Acknowledgement {
    digest: Vec<u8>,
    at: Instant,
    /// The local snapshot generation at which the digest was last found current.
    generation: u64,
}

struct PeerState {
    status: PeerStatus,
    last_success: Option<Instant>,
    last_attempt: Option<Instant>,
    next_attempt: Instant,
    acknowledged: Option<Acknowledgement>,
    /// An import from this peer lacked facts this replica holds, and no attempt
    /// to this peer has started since.
    push_back_requested: bool,
    /// When an attempt last served a push-back.
    last_push_back: Option<Instant>,
    /// Push-back requests so far. An attempt that overlapped one does not record
    /// its acknowledgement: the peer may lack facts that attempt did not carry.
    push_back_requests: u64,
}

impl PeerState {
    /// When the next attempt is due: at the scheduled time, or at once after a
    /// push-back, at most once per interval.
    fn due(&self, now: Instant, interval: Duration) -> Instant {
        if !self.push_back_requested {
            return self.next_attempt;
        }
        let push_back = self
            .last_push_back
            .map_or(now, |at| (at + interval).max(now));
        self.next_attempt.min(push_back)
    }
}

type PeerStates = BTreeMap<String, PeerState>;

struct Shared {
    stopping: AtomicBool,
    active: AtomicUsize,
    peak: AtomicUsize,
    rejected: AtomicUsize,
    peers: Mutex<PeerStates>,
    append_job_active: AtomicBool,
    append_worker_failures: AtomicUsize,
    catch_up: Mutex<CatchUp>,
    /// Counts the changes this process made to its local snapshot: appends and
    /// imports that inserted facts. An acknowledgement recorded at an older
    /// generation no longer describes the current snapshot.
    snapshot_generation: AtomicU64,
}

struct AppendJobGuard(Arc<Shared>);

impl Drop for AppendJobGuard {
    fn drop(&mut self) {
        self.0.append_job_active.store(false, Ordering::SeqCst);
    }
}

fn store(config: &Configuration) -> Result<Store> {
    Ok(Store::open(
        &config.network.database_path,
        config.network.manager.clone(),
        &config.network.replica_id,
    )?)
}

fn status(config: &Configuration, shared: &Shared) -> Result<Status> {
    let now = Instant::now();
    let refresh = config.unchanged_snapshot_refresh();
    let interval = Duration::from_millis(config.interval_ms);
    let generation = shared.snapshot_generation.load(Ordering::SeqCst);
    let peers = shared
        .peers
        .lock()
        .map_err(|_| "status lock poisoned")?
        .iter()
        .map(|(id, state)| {
            let mut status = state.status.clone();
            status.last_success_age_ms = state
                .last_success
                .map(|at| millis(now.saturating_duration_since(at)));
            status.last_attempt_age_ms = state
                .last_attempt
                .map(|at| millis(now.saturating_duration_since(at)));
            status.next_attempt_in_ms =
                millis(state.due(now, interval).saturating_duration_since(now));
            status.acknowledged_unchanged = !state.push_back_requested
                && state.acknowledged.as_ref().is_some_and(|acknowledgement| {
                    acknowledgement.generation == generation
                        && now.saturating_duration_since(acknowledgement.at) < refresh
                });
            status.refresh_ms = millis(refresh);
            status.max_backoff_ms = config.max_backoff_ms;
            (id.clone(), status)
        })
        .collect();
    let catch_up = shared
        .catch_up
        .lock()
        .map_err(|_| "catch-up lock poisoned")?
        .status(now);
    Ok(Status {
        kind: "resident_observation",
        replica_id: config.network.replica_id.clone(),
        canonical_inspection_available_via: "--inspect-store",
        peers,
        catch_up,
        active_incoming: shared.active.load(Ordering::SeqCst),
        peak_incoming: shared.peak.load(Ordering::SeqCst),
        rejected_connections: shared.rejected.load(Ordering::SeqCst),
        append_worker_failures: shared.append_worker_failures.load(Ordering::SeqCst),
        incoming_limit: config.incoming_workers,
        outgoing_limit: 1,
        activation_authority: false,
    })
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Control {
    Status {},
    Shutdown {},
    AppendObservation {
        operation_id: String,
        scope: String,
        subject: String,
        value: String,
    },
}

fn validate_control_token(value: &str) -> Result<()> {
    if (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Ok(())
    } else {
        Err("control token must be 1-128 safe ASCII characters".into())
    }
}

fn validate_control_scope(value: &str) -> Result<()> {
    if !(1..=128).contains(&value.len())
        || value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|segment| {
            segment.is_empty()
                || matches!(segment, "." | "..")
                || !segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
                })
        })
    {
        return Err("control scope must be a bounded safe hierarchical token".into());
    }
    Ok(())
}

fn validate_control_value(value: &str) -> Result<()> {
    if !value.is_empty() && value.len() <= 4096 {
        Ok(())
    } else {
        Err("control value must be nonempty UTF-8 and at most 4096 bytes".into())
    }
}

fn authorize_append_observation(
    config: &Configuration,
    stream: &UnixStream,
    operation_id: &str,
    scope: &str,
    subject: &str,
    value: &str,
) -> Result<()> {
    validate_control_token(operation_id)?;
    if operation_id.starts_with("network:") {
        return Err("network operation IDs are reserved".into());
    }
    validate_control_scope(scope)?;
    validate_control_token(subject)?;
    validate_control_value(value)?;
    let peer = rustix::net::sockopt::socket_peercred(stream)?;
    if peer.uid.as_raw() != config.observation_writer_uid {
        return Err("observation writer UID refused".into());
    }
    Ok(())
}

fn append_observation_store(
    config: &Configuration,
    shared: &Shared,
    operation_id: String,
    scope: String,
    subject: String,
    value: String,
) -> AppendCompletion {
    let mut store = match store(config) {
        Ok(store) => store,
        Err(_) => return AppendCompletion::Uncertain,
    };
    let executed = match store.execute_with_receipt(&Request::Observe {
        operation_id,
        scope,
        subject,
        exclusive_resource: None,
        active_claim: false,
        value,
    }) {
        Ok(executed) => executed,
        Err(DurableError::Refused(_)) => return AppendCompletion::Refused,
        Err(
            DurableError::Corrupt(_) | DurableError::Storage(_) | DurableError::InvalidAudit(_),
        ) => return AppendCompletion::Uncertain,
    };
    if !executed.replayed {
        shared.snapshot_generation.fetch_add(1, Ordering::SeqCst);
    }
    match serde_json::to_vec(&executed) {
        Ok(response) => AppendCompletion::Observed(response),
        Err(_) => AppendCompletion::Uncertain,
    }
}

fn start_append_observation(
    config: &Configuration,
    shared: &Arc<Shared>,
    stream: &UnixStream,
    operation_id: String,
    scope: String,
    subject: String,
    value: String,
) -> std::result::Result<AppendJob, AppendStartError> {
    // This authorization and validation completes before the worker can call
    // Store::open. A concurrent request receives a typed busy result rather than being queued
    // behind a slow SQLite transaction, so the control loop remains responsive.
    authorize_append_observation(config, stream, &operation_id, &scope, &subject, &value)
        .map_err(|_| AppendStartError::Refused)?;
    // Until this process has caught up with its peers, an append is answered
    // without touching the store.
    if !shared
        .catch_up
        .lock()
        .is_ok_and(|mut catch_up| catch_up.evaluate(Instant::now()))
    {
        return Err(AppendStartError::CatchingUp);
    }
    if shared
        .append_job_active
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppendStartError::Busy);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let config = config.clone();
    let state = Arc::clone(shared);
    let worker = thread::spawn(move || {
        let _active = AppendJobGuard(Arc::clone(&state));
        let completion =
            append_observation_store(&config, &state, operation_id, scope, subject, value);
        let _ = sender.send(completion);
    });
    Ok((receiver, worker))
}

/// Accounts for what this replica durably decided on an authenticated request.
fn record_served_decision(shared: &Shared, decision: &ServedDecision) {
    match decision {
        ServedDecision::Imported(import) => record_served_import(shared, import),
        ServedDecision::Refused(refusal) => record_refused_import(shared, refusal),
    }
}

/// Accounts for an authenticated import from a peer. That peer is caught up
/// with, and when it lacked facts this replica holds, a push to it is due at once.
fn record_served_import(shared: &Shared, import: &ServedImport) {
    if !import.replayed && import.inserted > 0 {
        shared.snapshot_generation.fetch_add(1, Ordering::SeqCst);
    }
    if let Ok(mut catch_up) = shared.catch_up.lock() {
        catch_up.record_import(&import.source_replica_id, Instant::now());
    }
    // After a committed import this replica holds every fact of the snapshot, so
    // a longer history holds facts the peer lacks. A replay reports the counts of
    // its original commit, which say nothing about what the peer lacks now.
    if import.replayed || import.history_len <= import.snapshot_facts {
        return;
    }
    if let Ok(mut peers) = shared.peers.lock() {
        if let Some(state) = peers.get_mut(&import.source_replica_id) {
            state.acknowledged = None;
            state.push_back_requested = true;
            state.push_back_requests = state.push_back_requests.saturating_add(1);
        }
    }
}

/// Counts an authenticated import from a peer that this replica refused and,
/// when the refused snapshot held an event ID this replica holds with other
/// bytes, the collision, named once on standard error for each new colliding
/// event ID from that peer.
fn record_refused_import(shared: &Shared, refusal: &ServedRefusal) {
    let newly_named = {
        let Ok(mut peers) = shared.peers.lock() else {
            return;
        };
        let Some(state) = peers.get_mut(&refusal.source_replica_id) else {
            return;
        };
        state.status.refused_imports = state.status.refused_imports.saturating_add(1);
        if refusal.identity_collisions == 0 {
            return;
        }
        let collisions = &mut state.status.identity_collisions;
        collisions.imports_refused = collisions.imports_refused.saturating_add(1);
        let newly_named = collisions.event_id != refusal.first_identity_collision;
        collisions
            .event_id
            .clone_from(&refusal.first_identity_collision);
        newly_named
    };
    if newly_named {
        eprintln!(
            "resident refused an import from {}: event identity collision on {} ({} of its facts held here with other bytes)",
            refusal.source_replica_id,
            refusal.first_identity_collision.as_deref().unwrap_or("an event"),
            refusal.identity_collisions
        );
    }
}

/// Runs until the private typed control interface requests graceful shutdown.
pub(crate) fn run(config: Configuration) -> Result<()> {
    cli::require_network_mode()?;
    config.validate()?;
    // Holding the lock prevents duplicate residents on this configured database.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config.network.database_path.with_extension("resident-lock"))?;
    lock.try_lock_exclusive()?;
    let _validated = config.network.open()?;
    // How this process catches up with its peers depends on whether its store
    // already held facts of this replica's own origin.
    let own_facts_at_start = match store(&config)?.execute(&Request::Export {})? {
        Response::Snapshot { snapshot } => snapshot
            .facts
            .iter()
            .filter(|fact| fact.origin_replica_id == config.network.replica_id)
            .count(),
        _ => return Err("durable manager returned an unexpected export response".into()),
    };
    let listener = TcpListener::bind(config.network.bind)?;
    listener.set_nonblocking(true)?;
    // Existing paths, including stale sockets after a crash, are refused.
    let control = UnixListener::bind(&config.control_socket)?;
    fs::set_permissions(&config.control_socket, fs::Permissions::from_mode(0o600))?;
    control.set_nonblocking(true)?;
    let shared = Arc::new(Shared {
        stopping: AtomicBool::new(false),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        rejected: AtomicUsize::new(0),
        append_job_active: AtomicBool::new(false),
        append_worker_failures: AtomicUsize::new(0),
        peers: Mutex::new(
            config
                .network
                .peers
                .iter()
                .map(|peer| {
                    (
                        peer.replica_id.clone(),
                        PeerState {
                            status: PeerStatus {
                                outcome: "unknown".into(),
                                ..PeerStatus::default()
                            },
                            last_success: None,
                            last_attempt: None,
                            next_attempt: Instant::now(),
                            acknowledged: None,
                            push_back_requested: false,
                            last_push_back: None,
                            push_back_requests: 0,
                        },
                    )
                })
                .collect(),
        ),
        catch_up: Mutex::new(CatchUp::new(
            config
                .network
                .peers
                .iter()
                .map(|peer| peer.replica_id.clone())
                .collect(),
            own_facts_at_start,
            config.catch_up_window(),
        )),
        snapshot_generation: AtomicU64::new(0),
    });
    let outgoing_config = config.clone();
    let outgoing_shared = Arc::clone(&shared);
    let outgoing = thread::spawn(move || synchronize(&outgoing_config, &outgoing_shared));
    // The open above verified the whole store once for this process; this worker
    // repeats that verification in the background at the configured interval.
    let integrity_config = config.clone();
    let integrity_shared = Arc::clone(&shared);
    let integrity =
        thread::spawn(move || verify_store_periodically(&integrity_config, &integrity_shared));
    let mut workers = Vec::new();
    let mut append_workers = Vec::new();
    let result = (|| -> Result<()> {
        while !shared.stopping.load(Ordering::SeqCst) {
            if outgoing.is_finished() {
                return Err("replication worker stopped unexpectedly".into());
            }
            // The periodic verification is what bounds the detection of an edited
            // old row; a resident whose verification worker stopped must not serve.
            if integrity.is_finished() {
                return Err("store verification worker stopped unexpectedly".into());
            }
            let mut index = 0;
            while index < workers.len() {
                if thread::JoinHandle::is_finished(&workers[index]) {
                    workers
                        .swap_remove(index)
                        .join()
                        .map_err(|_| "incoming worker panicked")?;
                } else {
                    index += 1;
                }
            }
            let mut append_index = 0;
            while append_index < append_workers.len() {
                if thread::JoinHandle::is_finished(&append_workers[append_index]) {
                    let worker = append_workers.swap_remove(append_index);
                    if worker.join().is_err() {
                        shared.append_worker_failures.fetch_add(1, Ordering::SeqCst);
                    }
                } else {
                    append_index += 1;
                }
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if workers.len() >= config.incoming_workers {
                        shared.rejected.fetch_add(1, Ordering::SeqCst);
                        drop(stream);
                    } else {
                        let conf = config.clone();
                        let state = Arc::clone(&shared);
                        let count = state.active.fetch_add(1, Ordering::SeqCst) + 1;
                        state.peak.fetch_max(count, Ordering::SeqCst);
                        workers.push(thread::spawn(move || {
                            if let Ok(mut node) = conf.network.open() {
                                // Recorded as soon as the decision is durable, before
                                // its reply: an import has already changed the snapshot.
                                let _ = node.serve_connection_reporting(stream, |decision| {
                                    record_served_decision(&state, decision);
                                });
                            }
                            state.active.fetch_sub(1, Ordering::SeqCst);
                        }));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            match control.accept() {
                Ok((mut stream, _)) => {
                    let deadline = Instant::now() + CONTROL_DEADLINE;
                    stream.set_read_timeout(Some(CONTROL_DEADLINE))?;
                    stream.set_write_timeout(Some(CONTROL_DEADLINE))?;
                    let response = match read_control(&mut stream, deadline) {
                        Ok(bytes) => match serde_json::from_slice::<Control>(&bytes) {
                            Ok(Control::Status {}) => match status(&config, &shared) {
                                Ok(status) => serde_json::to_vec(&status).unwrap_or_else(|_| {
                                    b"{\"error\":\"status_unavailable\"}".to_vec()
                                }),
                                Err(_) => b"{\"error\":\"status_unavailable\"}".to_vec(),
                            },
                            Ok(Control::Shutdown {}) => {
                                shared.stopping.store(true, Ordering::SeqCst);
                                b"{\"shutdown_requested\":true}".to_vec()
                            }
                            Ok(Control::AppendObservation {
                                operation_id,
                                scope,
                                subject,
                                value,
                            }) => match start_append_observation(
                                &config,
                                &shared,
                                &stream,
                                operation_id,
                                scope,
                                subject,
                                value,
                            ) {
                                Ok((receiver, worker)) => {
                                    let wait = deadline
                                        .saturating_duration_since(Instant::now())
                                        .saturating_sub(CONTROL_WRITE_RESERVE);
                                    match receiver.recv_timeout(wait) {
                                        Ok(AppendCompletion::Observed(response)) => {
                                            let _ = worker.join();
                                            response
                                        }
                                        Ok(AppendCompletion::Refused) => {
                                            let _ = worker.join();
                                            b"{\"error\":\"append_observation_refused\"}".to_vec()
                                        }
                                        Ok(AppendCompletion::Uncertain) => {
                                            let _ = worker.join();
                                            b"{\"error\":\"append_observation_uncertain\"}".to_vec()
                                        }
                                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                                            if worker.join().is_err() {
                                                shared
                                                    .append_worker_failures
                                                    .fetch_add(1, Ordering::SeqCst);
                                            }
                                            b"{\"error\":\"append_observation_uncertain\"}".to_vec()
                                        }
                                        Err(mpsc::RecvTimeoutError::Timeout) => {
                                            append_workers.push(worker);
                                            b"{\"error\":\"append_observation_uncertain\"}".to_vec()
                                        }
                                    }
                                }
                                Err(AppendStartError::Busy) => {
                                    b"{\"error\":\"append_observation_busy\"}".to_vec()
                                }
                                Err(AppendStartError::CatchingUp) => {
                                    b"{\"error\":\"append_observation_catching_up\"}".to_vec()
                                }
                                Err(AppendStartError::Refused) => {
                                    b"{\"error\":\"append_observation_refused\"}".to_vec()
                                }
                            },
                            Err(_) => b"{\"error\":\"invalid typed control request\"}".to_vec(),
                        },
                        _ => b"{\"error\":\"control request bound exceeded\"}".to_vec(),
                    };
                    if response.len() <= CONTROL_RESPONSE_MAX {
                        let _ = write_control(&mut stream, &response, deadline);
                    } else {
                        let _ = write_control(
                            &mut stream,
                            b"{\"error\":\"control response exceeds bound\"}",
                            deadline,
                        );
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    })();
    shared.stopping.store(true, Ordering::SeqCst);
    // Stop admission and unlink the socket we successfully created before any
    // fallible joins, including failures from status/store/outgoing operations.
    drop(control);
    drop(listener);
    let cleanup = fs::remove_file(&config.control_socket);
    let mut join_failed = false;
    for worker in workers {
        join_failed |= worker.join().is_err();
    }
    for worker in append_workers {
        if worker.join().is_err() {
            shared.append_worker_failures.fetch_add(1, Ordering::SeqCst);
        }
    }
    let outgoing_result = outgoing.join();
    let integrity_result = integrity.join();
    cleanup?;
    if join_failed {
        return Err("incoming worker panicked".into());
    }
    integrity_result.map_err(|_| "store verification worker panicked")?;
    outgoing_result.map_err(|_| "outgoing worker panicked")??;
    result
}

const CONTROL_DEADLINE: Duration = Duration::from_millis(250);
const CONTROL_WRITE_RESERVE: Duration = Duration::from_millis(25);
const CONTROL_RESPONSE_MAX: usize = 32_768;

fn read_control(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 257];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("control deadline exceeded")?;
        stream.set_read_timeout(Some(remaining))?;
        let read = match stream.read(&mut chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > 32_768 {
            return Err("control size exceeded".into());
        }
    }
}

fn write_control(stream: &mut UnixStream, mut bytes: &[u8], deadline: Instant) -> Result<()> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("control deadline exceeded")?;
        stream.set_write_timeout(Some(remaining))?;
        let written = match stream.write(bytes) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if written == 0 {
            return Err("control write stopped".into());
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

/// Why a periodic pass did not verify the store.
enum PassFailure {
    /// The store failed verification, now or earlier in this process, and stays
    /// closed for the rest of the process.
    Verification(DurableError),
    /// The pass could not run: the store did not open, or the pass could not
    /// take its snapshot. Nothing was verified and nothing was closed.
    NotRun(DurableError),
}

fn verify_store_once(config: &Configuration) -> std::result::Result<(), PassFailure> {
    let mut store = Store::open(
        &config.network.database_path,
        config.network.manager.clone(),
        &config.network.replica_id,
    )
    .map_err(|problem| match problem {
        DurableError::Corrupt(_) => PassFailure::Verification(problem),
        other => PassFailure::NotRun(other),
    })?;
    store.verify_full().map_err(|problem| {
        // A storage failure once the snapshot was held also closes the store.
        let closed = matches!(problem, DurableError::Corrupt(_))
            || store
                .integrity()
                .is_ok_and(|integrity| integrity.failure.is_some());
        if closed {
            PassFailure::Verification(problem)
        } else {
            PassFailure::NotRun(problem)
        }
    })
}

/// Repeats the complete store verification at the configured interval, in a read
/// transaction that does not block writers. A failed verification fails the store
/// closed for this process: every later store open, append, import, audit and
/// export returns that failure, while status and shutdown stay available. A pass
/// that could not run is retried after a backoff that starts at the exchange
/// interval and doubles up to the maximum backoff, never later than the next
/// regular pass would have run, so the detection interval does not silently grow.
fn verify_store_periodically(config: &Configuration, shared: &Shared) {
    let interval = config.full_verification_interval();
    let first_retry = Duration::from_millis(config.interval_ms).min(interval);
    let last_retry = Duration::from_millis(config.max_backoff_ms).min(interval);
    let mut retry = first_retry;
    let mut next = Instant::now() + interval;
    while !shared.stopping.load(Ordering::SeqCst) {
        if Instant::now() >= next {
            match verify_store_once(config) {
                Ok(()) => {
                    retry = first_retry;
                    next = Instant::now() + interval;
                }
                Err(PassFailure::Verification(problem)) => {
                    eprintln!("resident store verification failed: {problem}");
                    next = Instant::now() + interval;
                }
                Err(PassFailure::NotRun(problem)) => {
                    eprintln!(
                        "resident store verification could not run: {problem}; retrying in {} ms",
                        millis(retry)
                    );
                    next = Instant::now() + retry;
                    retry = retry.saturating_mul(2).min(last_retry);
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn record_local_failure(
    config: &Configuration,
    shared: &Shared,
    backoffs: &mut BTreeMap<String, u64>,
    peer_id: &str,
    local_history_len: Option<usize>,
) -> Result<()> {
    let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
    let state = peers.get_mut(peer_id).ok_or("unknown peer")?;
    let now = Instant::now();
    state.status.local_history_len_at_attempt = local_history_len;
    state.status.history_count_delta = None;
    state.status.failures = state.status.failures.saturating_add(1);
    state.status.outcome = "local_exchange_failure".into();
    state.last_attempt = Some(now);
    let backoff = backoffs.entry(peer_id.into()).or_insert(config.interval_ms);
    *backoff = backoff.saturating_mul(2).min(config.max_backoff_ms);
    state.next_attempt = now + Duration::from_millis(*backoff);
    Ok(())
}

fn synchronize(config: &Configuration, shared: &Shared) -> Result<()> {
    let mut backoffs = BTreeMap::new();
    let refresh = config.unchanged_snapshot_refresh();
    let interval = Duration::from_millis(config.interval_ms);
    // Reuse one operation ID for an unchanged observed snapshot. Fresh IDs after
    // restart are safe but receipts are not compacted by this laboratory.
    let mut operations: BTreeMap<String, (Vec<u8>, String)> = BTreeMap::new();
    while !shared.stopping.load(Ordering::SeqCst) {
        for peer in &config.network.peers {
            if shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            let push_back_requests = {
                let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
                let state = peers.get_mut(&peer.replica_id).ok_or("unknown peer")?;
                let now = Instant::now();
                if now < state.due(now, interval) {
                    continue;
                }
                // This attempt serves a requested push-back, whether or not it was
                // due anyway: the forgotten acknowledgement makes it push.
                if state.push_back_requested {
                    state.push_back_requested = false;
                    state.last_push_back = Some(now);
                    state.status.push_backs = state.status.push_backs.saturating_add(1);
                }
                state.push_back_requests
            };
            // Read before the export, so that a change racing it leaves the
            // acknowledgement at an older generation rather than a newer one.
            let generation = shared.snapshot_generation.load(Ordering::SeqCst);
            // A live SQLite snapshot can transiently refuse an open while another
            // bounded operation is committing. That is a peer-attempt failure, not
            // a resident-fatal condition: leave the control service available and
            // retry it through the existing bounded backoff.
            let local = match store(config)
                .and_then(|mut store| Ok(store.execute(&Request::Export {})?))
            {
                Ok(Response::Snapshot { snapshot }) => snapshot,
                Ok(_) | Err(_) => {
                    record_local_failure(config, shared, &mut backoffs, &peer.replica_id, None)?;
                    continue;
                }
            };
            let digest = Sha256::digest(serde_json::to_vec(&local)?).to_vec();
            {
                // The digest each peer last acknowledged with an authenticated
                // receipt, and when. An unchanged snapshot is not exchanged again
                // before the refresh delay, so an idle replica adds no audit rows
                // every interval.
                let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
                let state = peers.get_mut(&peer.replica_id).ok_or("unknown peer")?;
                if let Some(acknowledgement) = state
                    .acknowledged
                    .as_mut()
                    .filter(|known| known.digest == digest && known.at.elapsed() < refresh)
                {
                    acknowledgement.generation = generation;
                    state.next_attempt = Instant::now() + interval;
                    continue;
                }
            }
            let operation = operations
                .entry(peer.replica_id.clone())
                .or_insert_with(|| (Vec::new(), String::new()));
            if operation.0 != digest {
                *operation = (digest.clone(), random_token()?);
            }
            let outcome = match config.network.open() {
                Ok(mut node) => node.sync_to(&peer.replica_id, &operation.1, &random_token()?),
                Err(_) => {
                    record_local_failure(
                        config,
                        shared,
                        &mut backoffs,
                        &peer.replica_id,
                        Some(local.facts.len()),
                    )?;
                    continue;
                }
            };
            let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
            let state = peers.get_mut(&peer.replica_id).ok_or("unknown peer")?;
            let finished = Instant::now();
            state.last_attempt = Some(finished);
            state.status.local_history_len_at_attempt = Some(local.facts.len());
            let backoff = backoffs
                .entry(peer.replica_id.clone())
                .or_insert(config.interval_ms);
            let mut matched = false;
            match outcome {
                Ok(receipt) => {
                    state.status.authenticated_successes =
                        state.status.authenticated_successes.saturating_add(1);
                    state.status.acknowledged_history_len = Some(receipt.history_len);
                    state.status.history_count_delta =
                        Some(receipt.history_len as i128 - local.facts.len() as i128);
                    state.status.outcome = "authenticated_import_receipt".into();
                    state.last_success = Some(finished);
                    *backoff = config.interval_ms;
                    // A push-back requested while this attempt ran leaves the
                    // acknowledgement unrecorded, so the requested push follows.
                    state.acknowledged = (state.push_back_requests == push_back_requests)
                        .then_some(Acknowledgement {
                            digest,
                            at: finished,
                            generation,
                        });
                    // A peer that imported the pushed facts and counts exactly as
                    // many held no fact this replica lacked. The transport exports
                    // again, and a history only grows, so a count equal to this
                    // export's proves both exports and the peer's history equal.
                    matched = receipt.history_len == local.facts.len();
                }
                Err(error) => {
                    // The last acknowledgement is historical. Do not compare
                    // it to a fresh local count as if the failed peer were current.
                    state.status.history_count_delta = None;
                    state.status.failures = state.status.failures.saturating_add(1);
                    if let Some(reason) = error.refusal_reason() {
                        state.status.authenticated_refusals =
                            state.status.authenticated_refusals.saturating_add(1);
                        state.status.last_refusal_reason = Some(reason);
                        let collisions = &mut state.status.identity_collisions;
                        if reason == RefusalReason::PolicyViolation
                            && collisions.imports_refused > 0
                        {
                            collisions.pushes_refused = collisions.pushes_refused.saturating_add(1);
                        }
                    }
                    state.status.outcome = match error.source() {
                        ErrorSource::UnauthenticatedRemoteDiagnostic => {
                            "unauthenticated_remote_diagnostic"
                        }
                        ErrorSource::AuthenticatedRemoteRefusal => "authenticated_remote_refusal",
                        ErrorSource::Local => "local_exchange_failure",
                    }
                    .into();
                    *backoff = backoff.saturating_mul(2).min(config.max_backoff_ms);
                }
            }
            state.next_attempt = finished + Duration::from_millis(*backoff);
            drop(peers);
            if matched {
                shared
                    .catch_up
                    .lock()
                    .map_err(|_| "catch-up lock poisoned")?
                    .record_match(&peer.replica_id, finished);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn random_token() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "OS randomness unavailable")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
