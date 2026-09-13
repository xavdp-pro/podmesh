//! Bounded resident replication laboratory. No executable control authority.
use fs2::FileExt;
use podmesh_manager_ha_lab::durable::{DurableError, Request, Response, Store};
use podmesh_manager_network_lab::{ConfigurationFile, ErrorSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::TcpListener,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
}

pub mod cli;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub network: ConfigurationFile,
    pub control_socket: PathBuf,
    pub observation_writer_uid: u32,
    pub interval_ms: u64,
    pub max_backoff_ms: u64,
    pub incoming_workers: usize,
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
    pub acknowledged_history_len: Option<usize>,
    pub local_history_len_at_attempt: Option<usize>,
    pub history_count_delta: Option<i128>,
    pub outcome: String,
    pub next_attempt_in_ms: u64,
}

#[derive(Serialize)]
struct Status {
    kind: &'static str,
    replica_id: String,
    canonical_inspection_available_via: &'static str,
    peers: BTreeMap<String, PeerStatus>,
    active_incoming: usize,
    peak_incoming: usize,
    rejected_connections: usize,
    append_worker_failures: usize,
    incoming_limit: usize,
    outgoing_limit: usize,
    activation_authority: bool,
}

type PeerStates = BTreeMap<String, (PeerStatus, Option<Instant>, Instant)>;

struct Shared {
    stopping: AtomicBool,
    active: AtomicUsize,
    peak: AtomicUsize,
    rejected: AtomicUsize,
    peers: Mutex<PeerStates>,
    append_job_active: AtomicBool,
    append_worker_failures: AtomicUsize,
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
    let peers = shared
        .peers
        .lock()
        .map_err(|_| "status lock poisoned")?
        .iter()
        .map(|(id, (status, success, next))| {
            let mut status = status.clone();
            status.last_success_age_ms =
                success.map(|at| millis(now.saturating_duration_since(at)));
            status.next_attempt_in_ms = millis(next.saturating_duration_since(now));
            (id.clone(), status)
        })
        .collect();
    Ok(Status {
        kind: "resident_observation",
        replica_id: config.network.replica_id.clone(),
        canonical_inspection_available_via: "--inspect-store",
        peers,
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
        let _active = AppendJobGuard(state);
        let completion = append_observation_store(&config, operation_id, scope, subject, value);
        let _ = sender.send(completion);
    });
    Ok((receiver, worker))
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
                        (
                            PeerStatus {
                                outcome: "unknown".into(),
                                ..PeerStatus::default()
                            },
                            None,
                            Instant::now(),
                        ),
                    )
                })
                .collect(),
        ),
    });
    let outgoing_config = config.clone();
    let outgoing_shared = Arc::clone(&shared);
    let outgoing = thread::spawn(move || synchronize(&outgoing_config, &outgoing_shared));
    let mut workers = Vec::new();
    let mut append_workers = Vec::new();
    let result = (|| -> Result<()> {
        while !shared.stopping.load(Ordering::SeqCst) {
            if outgoing.is_finished() {
                return Err("replication worker stopped unexpectedly".into());
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
                                let _ = node.serve_connection(stream);
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
    cleanup?;
    if join_failed {
        return Err("incoming worker panicked".into());
    }
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

fn record_local_failure(
    config: &Configuration,
    shared: &Shared,
    backoffs: &mut BTreeMap<String, u64>,
    peer_id: &str,
    local_history_len: Option<usize>,
) -> Result<()> {
    let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
    let (state, _, next) = peers.get_mut(peer_id).ok_or("unknown peer")?;
    state.local_history_len_at_attempt = local_history_len;
    state.history_count_delta = None;
    state.failures = state.failures.saturating_add(1);
    state.outcome = "local_exchange_failure".into();
    let backoff = backoffs.entry(peer_id.into()).or_insert(config.interval_ms);
    *backoff = backoff.saturating_mul(2).min(config.max_backoff_ms);
    *next = Instant::now() + Duration::from_millis(*backoff);
    Ok(())
}

fn synchronize(config: &Configuration, shared: &Shared) -> Result<()> {
    let mut backoffs = BTreeMap::new();
    // Reuse one operation ID for an unchanged observed snapshot. Fresh IDs after
    // restart are safe but receipts are not compacted by this laboratory.
    let mut operations: BTreeMap<String, (Vec<u8>, String)> = BTreeMap::new();
    while !shared.stopping.load(Ordering::SeqCst) {
        for peer in &config.network.peers {
            if shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            let due = shared.peers.lock().map_err(|_| "status lock poisoned")?[&peer.replica_id].2;
            if Instant::now() < due {
                continue;
            }
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
            let operation = operations
                .entry(peer.replica_id.clone())
                .or_insert_with(|| (Vec::new(), String::new()));
            if operation.0 != digest {
                *operation = (digest, random_token()?);
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
            let (state, success, next) = peers.get_mut(&peer.replica_id).ok_or("unknown peer")?;
            state.local_history_len_at_attempt = Some(local.facts.len());
            let backoff = backoffs
                .entry(peer.replica_id.clone())
                .or_insert(config.interval_ms);
            match outcome {
                Ok(receipt) => {
                    state.authenticated_successes = state.authenticated_successes.saturating_add(1);
                    state.acknowledged_history_len = Some(receipt.history_len);
                    state.history_count_delta =
                        Some(receipt.history_len as i128 - local.facts.len() as i128);
                    state.outcome = "authenticated_import_receipt".into();
                    *success = Some(Instant::now());
                    *backoff = config.interval_ms;
                }
                Err(error) => {
                    // The last acknowledgement is historical. Do not compare
                    // it to a fresh local count as if the failed peer were current.
                    state.history_count_delta = None;
                    state.failures = state.failures.saturating_add(1);
                    state.outcome = match error.source() {
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
            *next = Instant::now() + Duration::from_millis(*backoff);
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
