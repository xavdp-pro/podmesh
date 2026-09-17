//! Bounded resident replication laboratory. No executable control authority.
use fs2::FileExt;
use podmesh_manager_ha_lab::durable::{
    DurableError, RefusalReason, Request, Response, Store, StoreClosedState,
    DEFAULT_FULL_VERIFICATION_INTERVAL,
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
/// store's latest fact of its own origin was appended by the store itself waits
/// for peers it has not caught up with before it appends local facts anyway.
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
    /// Catch-up window of a store whose latest fact of this replica's own origin
    /// was appended by the store itself; absent means `DEFAULT_CATCH_UP_WINDOW`.
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
        // The window is not bounded by the universe's 25-second start budget any
        // more: that budget covers the store's first open and the control socket's
        // bind, and a replica that is catching up is running and not ready, waited
        // for with no deadline. What the cap bounds is how long a replica whose
        // peer is down stays unready: it appends its boot fact about one window
        // plus one round of attempts after it began exchanging (that round spends
        // a 2-second connect deadline on each peer that does not answer), so at
        // 15,000 ms it is ready some 17 to 19 seconds after its first open, inside
        // the 30 seconds PodMesh observes a start for. A longer window would push a
        // readiness that is going to come out of that observation; a shorter one
        // forgives an unreachable peer sooner, which is the hazard this window
        // trades against.
        if self
            .catch_up_window_ms
            .is_some_and(|window| !(1_000..=15_000).contains(&window))
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

    /// Catch-up window of a store whose latest own fact it appended itself.
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
    /// Every peer was caught up with.
    EveryPeer,
    /// The window ended for a store whose latest fact of its own origin it had
    /// appended itself, and evidence taken after its end showed one peer caught
    /// up with and every other one unreached without being known to hold facts
    /// this replica lacks.
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
    peers_ahead: Vec<String>,
    peers_not_attempted: Vec<String>,
    own_facts_at_start: usize,
    latest_own_fact_appended_locally: bool,
    window_ms: u64,
    appends_observed: u64,
    /// Why this process is not ready, when it can tell. Absent once it is.
    blocked_by: Option<ReadinessBlocker>,
}

/// What keeps a process from catching up, as far as its own evidence says. It
/// separates a replica that is waiting for a peer, and will be ready when that
/// peer answers, from one that never will be: an event identity collision is a
/// forked history, and no amount of waiting resolves it.
#[derive(Serialize)]
struct ReadinessBlocker {
    /// `identity_collision`: an import from one of the peers this process lacks
    /// was refused because its snapshot held an event ID this replica holds with
    /// other bytes, so no import from it will ever carry the facts this process
    /// needs. `refused_by_peer`: such a peer answers authenticated refusals
    /// without a collision found here, a policy question an operator settles.
    /// `waiting_for_peers`: the ordinary case, peers not reached or not caught
    /// up with yet.
    reason: &'static str,
    /// The peers the reason is about.
    peers: Vec<String>,
    /// The colliding event ID, for `identity_collision`.
    event_id: Option<String>,
}

/// Reads the blocker from a catch-up state and the links of the peers it lacks.
fn readiness_blocker(
    catch_up: &CatchUpStatus,
    peers: &BTreeMap<String, PeerStatus>,
) -> Option<ReadinessBlocker> {
    if catch_up.caught_up {
        return None;
    }
    let missing = |keep: &dyn Fn(&PeerStatus) -> bool| -> Vec<String> {
        catch_up
            .peers_missing
            .iter()
            .filter(|peer| peers.get(*peer).is_some_and(keep))
            .cloned()
            .collect()
    };
    let collided = missing(&|link| link.identity_collisions.imports_refused > 0);
    if !collided.is_empty() {
        let event_id = collided
            .iter()
            .find_map(|peer| peers.get(peer)?.identity_collisions.event_id.clone());
        return Some(ReadinessBlocker {
            reason: "identity_collision",
            peers: collided,
            event_id,
        });
    }
    let refused = missing(&|link| link.last_refusal_reason.is_some());
    if !refused.is_empty() {
        return Some(ReadinessBlocker {
            reason: "refused_by_peer",
            peers: refused,
            event_id: None,
        });
    }
    Some(ReadinessBlocker {
        reason: "waiting_for_peers",
        peers: catch_up.peers_missing.clone(),
        event_id: None,
    })
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
    /// Whether the store is closed for this process, and why. What reads this
    /// status fails closed with the store: an administration surface refuses
    /// while it is true, and refuses too when this status does not arrive.
    store_closed: bool,
    store_closed_reason: Option<String>,
}

/// What this process has learnt about one peer while it catches up.
#[derive(Default)]
struct PeerCatchUp {
    /// The largest snapshot of an authenticated import from the peer that this
    /// process committed or replayed.
    imported_snapshot: Option<usize>,
    /// The latest receipt from the peer counted exactly the facts this process
    /// pushed: after importing them the peer held nothing this replica lacked.
    matched: bool,
    /// The largest history a receipt from the peer counted beyond the facts this
    /// process pushed: the peer held facts this replica lacked.
    ahead_at: Option<usize>,
    /// The earliest moment at which that history can have been counted.
    ahead_counted_after: Option<Instant>,
    /// The latest moment at which this replica is known to have held every fact
    /// the peer held: a receipt that counted exactly the pushed facts, or one
    /// that counted a history an import has since carried in full.
    covered_since: Option<Instant>,
    /// An attempt of this process to reach the peer has ended.
    attempted: bool,
    /// The start of the latest attempt, when it drew no authenticated answer.
    unreached_since: Option<Instant>,
}

impl PeerCatchUp {
    /// Whether this replica holds every fact the peer is known to have held. A
    /// history only grows, so an import carrying at least as many facts as the
    /// peer was known to hold carried all of them.
    fn caught_up(&self) -> bool {
        let known = self.ahead_at.unwrap_or(0);
        self.matched || self.imported_snapshot.is_some_and(|facts| facts >= known)
    }

    /// Whether the peer holds facts this replica lacks, as far as this process knows.
    fn ahead(&self) -> bool {
        self.ahead_at.is_some() && !self.caught_up()
    }

    /// Caught up with on evidence no older than `moment`.
    fn caught_up_after(&self, moment: Instant) -> bool {
        self.caught_up() && self.covered_since.is_some_and(|since| since >= moment)
    }

    /// Unreached by an attempt started at or after `moment`, and not known to
    /// hold facts this replica lacks.
    fn unreached_after(&self, moment: Instant) -> bool {
        !self.ahead() && self.unreached_since.is_some_and(|since| since >= moment)
    }
}

/// How one attempt to reach a peer ended.
enum AttemptOutcome {
    /// An authenticated receipt: the peer's history after importing the pushed
    /// facts, and the earliest moment at which it can have been counted.
    Receipt {
        history: usize,
        pushed: usize,
        counted_after: Instant,
    },
    /// A verified signed refusal: the peer was reached and refused.
    Refused,
    /// No authenticated answer.
    Unreached,
}

/// Whether this process has caught up with its peers, which it must have before
/// its first local append. A store that lacks some facts of this replica's own
/// origin, deleted or restored from an older copy, would otherwise append under
/// producer sequences its peers already hold with other bytes, and both sides
/// would then refuse every import from the other. Importing those facts back
/// from the peers first makes the next local fact take the next sequence.
///
/// A peer is caught up with once this process has committed or replayed an
/// authenticated import from it that carried every fact it was known to hold,
/// or once its authenticated receipt for a push of this process counted exactly
/// the pushed facts. A receipt counting more tells that the peer holds facts
/// this replica lacks, until such an import. Every peer caught up with catches
/// the process up.
///
/// The window forgives the others, only to a store whose latest fact of its own
/// origin was appended by the store itself (an emptied store that imported some
/// of its own facts back still waits for every peer, in every later process),
/// and only on evidence no older than the window's end: when the window ends,
/// every peer is made due again with a fresh operation, and the window applies
/// once one peer is caught up with on such evidence and every other one is
/// unreached by an attempt started after the end, none of them known to hold
/// facts this replica lacks. A peer caught up with earlier is not enough: it may
/// since have received such facts, relayed from a replica this process cannot
/// reach. A peer that answers an authenticated refusal is reached, not forgiven:
/// its exchange service works and its history stays unknown. A replica that
/// reaches no peer never appends. The state latches for the process.
struct CatchUp {
    /// When this process started exchanging.
    started: Instant,
    window: Duration,
    own_facts_at_start: usize,
    /// The latest fact of this replica's origin in the store at start was
    /// appended by the store itself, not imported back from a peer.
    latest_own_fact_appended_locally: bool,
    peers: BTreeMap<String, PeerCatchUp>,
    /// Every peer was made due again when the window ended.
    window_refreshed: bool,
    caught_up: Option<(CaughtUpBy, Instant)>,
    /// Appends this process answered observed, replays included.
    appends_observed: u64,
}

impl CatchUp {
    fn new(
        peers: impl IntoIterator<Item = String>,
        own_facts_at_start: usize,
        latest_own_fact_appended_locally: bool,
        window: Duration,
    ) -> Self {
        let started = Instant::now();
        let peers: BTreeMap<_, _> = peers
            .into_iter()
            .map(|peer| (peer, PeerCatchUp::default()))
            .collect();
        Self {
            caught_up: peers.is_empty().then_some((CaughtUpBy::NoPeers, started)),
            started,
            window,
            own_facts_at_start,
            latest_own_fact_appended_locally,
            peers,
            window_refreshed: false,
            appends_observed: 0,
        }
    }

    fn window_end(&self) -> Instant {
        self.started + self.window
    }

    /// Whether the window applies: every condition holds on evidence taken after
    /// its end, so it can only start to hold when such evidence arrives.
    fn window_applies(&self) -> bool {
        let end = self.window_end();
        self.latest_own_fact_appended_locally
            && self.peers.values().any(|peer| peer.caught_up_after(end))
            && self
                .peers
                .values()
                .all(|peer| peer.caught_up_after(end) || peer.unreached_after(end))
    }

    /// Whether this process may append, latching the first condition that held.
    fn evaluate(&mut self, now: Instant) -> bool {
        if self.caught_up.is_none() {
            if self.peers.values().all(PeerCatchUp::caught_up) {
                self.caught_up = Some((CaughtUpBy::EveryPeer, now));
            } else if self.window_applies() {
                self.caught_up = Some((CaughtUpBy::Window, now));
            }
        }
        self.caught_up.is_some()
    }

    /// Applies what one event taught about a peer, then evaluates: never the
    /// other way round, or a receipt showing a peer ahead could latch a window
    /// before it counts.
    fn learn(&mut self, peer: &str, now: Instant, update: impl FnOnce(&mut PeerCatchUp)) {
        if let Some(state) = self.peers.get_mut(peer) {
            update(state);
        }
        self.evaluate(now);
    }

    /// Records an authenticated import from the peer that this process committed
    /// or replayed. The import carries the peer's history as it was exported,
    /// which this process cannot date; what it dates is the history the import
    /// covers, counted by a receipt.
    fn record_import(&mut self, peer: &str, snapshot_facts: usize, now: Instant) {
        self.learn(peer, now, |state| {
            state.imported_snapshot = state.imported_snapshot.max(Some(snapshot_facts));
            if state
                .ahead_at
                .is_some_and(|history| snapshot_facts >= history)
            {
                state.covered_since = state.covered_since.max(state.ahead_counted_after);
            }
        });
    }

    /// Records how an attempt that reached the network, started at `started`, ended.
    fn record_attempt(
        &mut self,
        peer: &str,
        started: Instant,
        outcome: &AttemptOutcome,
        now: Instant,
    ) {
        self.learn(peer, now, |state| {
            state.attempted = true;
            state.unreached_since = None;
            match *outcome {
                // The peer's history, after importing the pushed facts, counted
                // exactly as many: it held nothing this replica lacked.
                AttemptOutcome::Receipt {
                    history,
                    pushed,
                    counted_after,
                } if history == pushed => {
                    state.matched = true;
                    state.ahead_at = None;
                    state.ahead_counted_after = None;
                    state.covered_since = state.covered_since.max(Some(counted_after));
                }
                // It counted more: the peer held facts this replica lacked, unless
                // an import has since carried a history at least as long, which a
                // history that only grows makes the same one.
                AttemptOutcome::Receipt {
                    history,
                    pushed,
                    counted_after,
                } if history > pushed => {
                    state.matched = false;
                    if Some(history) >= state.ahead_at {
                        state.ahead_at = Some(history);
                        state.ahead_counted_after =
                            state.ahead_counted_after.max(Some(counted_after));
                    }
                    if state
                        .imported_snapshot
                        .is_some_and(|facts| facts >= history)
                    {
                        state.covered_since = state.covered_since.max(Some(counted_after));
                    }
                }
                AttemptOutcome::Receipt { .. } | AttemptOutcome::Refused => {}
                AttemptOutcome::Unreached => state.unreached_since = Some(started),
            }
        });
    }

    /// True once, when the window has ended before this process caught up: every
    /// peer is then to be attempted again with a fresh operation, so that the
    /// window counts only evidence taken after its end.
    fn window_refresh_due(&mut self, now: Instant) -> bool {
        let due = !self.window_refreshed
            && self.caught_up.is_none()
            && self.latest_own_fact_appended_locally
            && now >= self.window_end();
        self.window_refreshed |= due;
        due
    }

    fn record_observed_append(&mut self) {
        self.appends_observed = self.appends_observed.saturating_add(1);
    }

    fn named(&self, keep: impl Fn(&PeerCatchUp) -> bool) -> Vec<String> {
        self.peers
            .iter()
            .filter(|(_, state)| keep(state))
            .map(|(peer, _)| peer.clone())
            .collect()
    }

    fn status(&mut self, now: Instant) -> CatchUpStatus {
        self.evaluate(now);
        CatchUpStatus {
            caught_up: self.caught_up.is_some(),
            caught_up_by: self.caught_up.map(|(by, _)| by),
            caught_up_after_ms: self
                .caught_up
                .map(|(_, at)| millis(at.saturating_duration_since(self.started))),
            peers_imported: self.named(|state| {
                state
                    .imported_snapshot
                    .is_some_and(|facts| facts >= state.ahead_at.unwrap_or(0))
            }),
            peers_matched: self.named(|state| state.matched),
            peers_missing: self.named(|state| !state.caught_up()),
            peers_ahead: self.named(PeerCatchUp::ahead),
            peers_not_attempted: self.named(|state| !state.caught_up() && !state.attempted),
            own_facts_at_start: self.own_facts_at_start,
            latest_own_fact_appended_locally: self.latest_own_fact_appended_locally,
            window_ms: millis(self.window),
            appends_observed: self.appends_observed,
            // Filled by the status, which also reads the peers' links.
            blocked_by: None,
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
    /// The colliding event IDs this process has already named on standard error
    /// for this peer.
    named_collisions: BTreeSet<String>,
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
    /// The closed state of the store's database file, read here rather than from
    /// the store itself: the store serves operations in another thread.
    store_closed_state: StoreClosedState,
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
    let mut catch_up = shared
        .catch_up
        .lock()
        .map_err(|_| "catch-up lock poisoned")?
        .status(now);
    catch_up.blocked_by = readiness_blocker(&catch_up, &peers);
    // A closed state that cannot be read counts as closed: a store whose state is
    // unknown is not an open one.
    let store_closed = match shared.store_closed_state.failure() {
        Ok(failure) => failure,
        Err(problem) => Some(problem),
    };
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
        store_closed: store_closed.is_some(),
        store_closed_reason: store_closed.map(|failure| failure.to_string()),
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
    let mut catching_up = false;
    if let Ok(mut catch_up) = shared.catch_up.lock() {
        catch_up.record_import(
            &import.source_replica_id,
            import.snapshot_facts,
            Instant::now(),
        );
        catching_up = catch_up.caught_up.is_none();
    }
    // This request shows that the peer is exchanging again. A process that has
    // not caught up needs a receipt of its own from it, dated, which no import
    // gives: attempt it at once rather than at the end of a long backoff.
    if catching_up {
        if let Ok(mut peers) = shared.peers.lock() {
            if let Some(state) = peers.get_mut(&import.source_replica_id) {
                state.next_attempt = Instant::now();
            }
        }
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
        state.status.identity_collisions.imports_refused = state
            .status
            .identity_collisions
            .imports_refused
            .saturating_add(1);
        state
            .status
            .identity_collisions
            .event_id
            .clone_from(&refusal.first_identity_collision);
        // One line for each colliding event ID this peer has shown, however often
        // its refusals repeat and whichever order they come in: the set is what
        // makes that true, since the status keeps only the latest ID.
        refusal
            .first_identity_collision
            .as_ref()
            .is_some_and(|event_id| state.named_collisions.insert(event_id.clone()))
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
    // How this process catches up with its peers depends on the store it found:
    // the facts of this replica's own origin it held, and whether the latest of
    // them was appended by the store itself rather than imported back from a peer.
    let mut startup_store = store(&config)?;
    let (own_facts_at_start, latest_own_sequence) =
        match startup_store.execute(&Request::Export {})? {
            Response::Snapshot { snapshot } => snapshot
                .facts
                .iter()
                .filter(|fact| fact.origin_replica_id == config.network.replica_id)
                .fold((0_usize, None::<u64>), |(count, latest), fact| {
                    (count + 1, latest.max(Some(fact.producer_sequence)))
                }),
            _ => return Err("durable manager returned an unexpected export response".into()),
        };
    let latest_own_fact_appended_locally = latest_own_sequence.is_some()
        && startup_store.highest_local_observation()? == latest_own_sequence;
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
                            named_collisions: BTreeSet::new(),
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
                .map(|peer| peer.replica_id.clone()),
            own_facts_at_start,
            latest_own_fact_appended_locally,
            config.catch_up_window(),
        )),
        snapshot_generation: AtomicU64::new(0),
        store_closed_state: startup_store.closed_state(),
    });
    let outgoing_config = config.clone();
    let outgoing_shared = Arc::clone(&shared);
    let outgoing = thread::spawn(move || synchronize(&outgoing_config, &outgoing_shared));
    // The open above verified the whole store once for this process; this worker
    // repeats that verification in the background at the configured interval.
    let integrity_config = config.clone();
    let integrity_shared = Arc::clone(&shared);
    let integrity = thread::spawn(move || {
        verify_store_periodically(&integrity_config, &integrity_shared, startup_store);
    });
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
                                            if let Ok(mut catch_up) = shared.catch_up.lock() {
                                                catch_up.record_observed_append();
                                            }
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
    /// A verification failure closed the store for the rest of the process, in
    /// this pass or earlier: later passes can only repeat it.
    Closed(DurableError),
    /// The pass could not run: the store did not open, or the pass could not
    /// take its snapshot. Nothing was verified and nothing was closed.
    NotRun(DurableError),
}

/// Runs one complete verification. `known` is the last store this worker opened:
/// its integrity state tells a store a verification failure closed, whose later
/// opens return that failure whatever its class, from a pass that could not run.
fn verify_store_once(
    config: &Configuration,
    known: &mut Store,
) -> std::result::Result<(), PassFailure> {
    let classify = |known: &Store, problem: DurableError| {
        let closed = matches!(problem, DurableError::Corrupt(_))
            || known
                .integrity()
                .is_ok_and(|integrity| integrity.failure.is_some());
        if closed {
            PassFailure::Closed(problem)
        } else {
            PassFailure::NotRun(problem)
        }
    };
    match Store::open(
        &config.network.database_path,
        config.network.manager.clone(),
        &config.network.replica_id,
    ) {
        Ok(mut store) => {
            let result = store.verify_full();
            *known = store;
            result.map_err(|problem| classify(known, problem))
        }
        Err(problem) => Err(classify(known, problem)),
    }
}

/// Repeats the complete store verification at the configured interval, in a read
/// transaction that does not block writers. A failed verification fails the store
/// closed for this process: every later store open, append, import, audit and
/// export returns that failure, while status and shutdown stay available; later
/// passes keep the interval and say the store is closed. A pass that could not
/// run is retried after a backoff that starts at the exchange interval and
/// doubles up to the maximum backoff, never later than the next regular pass
/// would have run, so the detection interval does not silently grow.
fn verify_store_periodically(config: &Configuration, shared: &Shared, mut known: Store) {
    let interval = config.full_verification_interval();
    let first_retry = Duration::from_millis(config.interval_ms).min(interval);
    let last_retry = Duration::from_millis(config.max_backoff_ms).min(interval);
    let mut retry = first_retry;
    let mut next = Instant::now() + interval;
    let mut closed = false;
    while !shared.stopping.load(Ordering::SeqCst) {
        if Instant::now() >= next {
            match verify_store_once(config, &mut known) {
                Ok(()) => {
                    closed = false;
                    retry = first_retry;
                    next = Instant::now() + interval;
                }
                Err(PassFailure::Closed(problem)) => {
                    if closed {
                        eprintln!("resident store is still closed for this process: {problem}");
                    } else {
                        eprintln!(
                            "resident store verification failed: {problem}; the store is closed for this process"
                        );
                    }
                    closed = true;
                    retry = first_retry;
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
    // Reuse one operation ID for an unchanged observed snapshot, remembering when
    // it was created: a replayed receipt counts a history no older than that.
    // Fresh IDs after restart are safe but receipts are not compacted by this
    // laboratory.
    let mut operations: BTreeMap<String, (Vec<u8>, String, Instant)> = BTreeMap::new();
    while !shared.stopping.load(Ordering::SeqCst) {
        for peer in &config.network.peers {
            if shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            // When the window ends before this process caught up, every peer is
            // attempted again at once with a fresh operation: the window counts
            // only evidence taken after its end.
            let refresh_due = shared
                .catch_up
                .lock()
                .map_err(|_| "catch-up lock poisoned")?
                .window_refresh_due(Instant::now());
            if refresh_due {
                let now = Instant::now();
                let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
                for state in peers.values_mut() {
                    state.acknowledged = None;
                    state.next_attempt = now;
                }
                drop(peers);
                // A peer that stayed down through the window is looked for again
                // from the exchange interval, not from the backoff it had reached.
                backoffs.clear();
                operations.clear();
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
                .or_insert_with(|| (Vec::new(), String::new(), Instant::now()));
            if operation.0 != digest {
                *operation = (digest.clone(), random_token()?, Instant::now());
            }
            let attempt_started = Instant::now();
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
            // A local failure before any connection says nothing about the peer.
            let attempt = match &outcome {
                Ok(receipt) => Some(AttemptOutcome::Receipt {
                    history: receipt.history_len,
                    pushed: receipt.snapshot_facts,
                    // A fresh receipt counts a history the peer committed during
                    // this attempt; a replay counts the one of its first commit,
                    // no older than the operation this process created.
                    counted_after: if receipt.replayed {
                        operation.2
                    } else {
                        attempt_started
                    },
                }),
                Err(error) if !error.connection_attempted() => None,
                Err(error) if error.source() == ErrorSource::AuthenticatedRemoteRefusal => {
                    Some(AttemptOutcome::Refused)
                }
                Err(_) => Some(AttemptOutcome::Unreached),
            };
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
            // A peer whose history, after importing the pushed facts, counts exactly
            // as many held nothing this replica lacked; one that counts more held
            // facts this replica lacked.
            if let Some(attempt) = attempt {
                shared
                    .catch_up
                    .lock()
                    .map_err(|_| "catch-up lock poisoned")?
                    .record_attempt(&peer.replica_id, attempt_started, &attempt, finished);
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
