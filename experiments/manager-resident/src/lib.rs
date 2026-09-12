//! Bounded resident replication laboratory. No executable control authority.
use fs2::FileExt;
use podmesh_manager_ha_lab::durable::{Request, Response, Store};
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
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub network: ConfigurationFile,
    pub control_socket: PathBuf,
    pub interval_ms: u64,
    pub max_backoff_ms: u64,
    pub incoming_workers: usize,
}

impl Configuration {
    pub fn validate(&self) -> Result<()> {
        if !(100..=60_000).contains(&self.interval_ms)
            || !(self.interval_ms..=300_000).contains(&self.max_backoff_ms)
            || !(1..=8).contains(&self.incoming_workers)
            || self.network.peers.len() > 15
        {
            return Err("invalid interval, backoff, workers or peer count".into());
        }
        if !self.network.database_path.is_absolute()
            || !self.control_socket.is_absolute()
            || self.control_socket.as_os_str().len() > 100
        {
            return Err("database and socket require bounded absolute local paths".into());
        }
        let parent = self.control_socket.parent().ok_or("socket has no parent")?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err("control socket parent must be a private directory".into());
        }
        if self.network.bind.port() == 0
            || self.network.peers.iter().any(|p| p.endpoint.port() == 0)
        {
            return Err("static endpoints require nonzero ports".into());
        }
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
    inspection: Response,
    peers: BTreeMap<String, PeerStatus>,
    active_incoming: usize,
    peak_incoming: usize,
    rejected_connections: usize,
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
        inspection: store(config)?.execute(&Request::Inspect {})?,
        peers,
        active_incoming: shared.active.load(Ordering::SeqCst),
        peak_incoming: shared.peak.load(Ordering::SeqCst),
        rejected_connections: shared.rejected.load(Ordering::SeqCst),
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
}

/// Runs until the private typed control interface requests graceful shutdown.
pub fn run(config: Configuration) -> Result<()> {
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
                    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
                    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                    let response = match read_control(&mut stream) {
                        Ok(bytes) => match serde_json::from_slice::<Control>(&bytes) {
                            Ok(Control::Status {}) => {
                                serde_json::to_vec(&status(&config, &shared)?)?
                            }
                            Ok(Control::Shutdown {}) => {
                                shared.stopping.store(true, Ordering::SeqCst);
                                b"{\"shutdown_requested\":true}".to_vec()
                            }
                            Err(_) => b"{\"error\":\"invalid typed control request\"}".to_vec(),
                        },
                        _ => b"{\"error\":\"control request bound exceeded\"}".to_vec(),
                    };
                    if response.len() <= 524_288 {
                        let _ = write_control(&mut stream, &response);
                    } else {
                        let _ = write_control(
                            &mut stream,
                            b"{\"error\":\"status exceeds response limit\"}",
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
    let outgoing_result = outgoing.join();
    cleanup?;
    if join_failed {
        return Err("incoming worker panicked".into());
    }
    outgoing_result.map_err(|_| "outgoing worker panicked")??;
    result
}

fn read_control(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_millis(250);
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
        if bytes.len() > 256 {
            return Err("control size exceeded".into());
        }
    }
}

fn write_control(stream: &mut UnixStream, mut bytes: &[u8]) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(250);
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
            let local = store(config)?.execute(&Request::Export {})?;
            let Response::Snapshot { snapshot } = &local else {
                return Err("invalid store export".into());
            };
            let digest = Sha256::digest(serde_json::to_vec(snapshot)?).to_vec();
            let operation = operations
                .entry(peer.replica_id.clone())
                .or_insert_with(|| (Vec::new(), String::new()));
            if operation.0 != digest {
                *operation = (digest, random_token()?);
            }
            let outcome =
                config
                    .network
                    .open()?
                    .sync_to(&peer.replica_id, &operation.1, &random_token()?);
            let mut peers = shared.peers.lock().map_err(|_| "status lock poisoned")?;
            let (state, success, next) = peers.get_mut(&peer.replica_id).ok_or("unknown peer")?;
            state.local_history_len_at_attempt = Some(snapshot.facts.len());
            let backoff = backoffs
                .entry(peer.replica_id.clone())
                .or_insert(config.interval_ms);
            match outcome {
                Ok(receipt) => {
                    state.authenticated_successes = state.authenticated_successes.saturating_add(1);
                    state.acknowledged_history_len = Some(receipt.history_len);
                    state.history_count_delta =
                        Some(receipt.history_len as i128 - snapshot.facts.len() as i128);
                    state.outcome = "authenticated_import_receipt".into();
                    *success = Some(Instant::now());
                    *backoff = config.interval_ms;
                }
                Err(error) => {
                    // The last acknowledgement is historical. Do not compare
                    // it to a fresh local count as if the failed peer were current.
                    state.history_count_delta = None;
                    state.failures = state.failures.saturating_add(1);
                    state.outcome =
                        if error.source() == ErrorSource::UnauthenticatedRemoteDiagnostic {
                            "unauthenticated_remote_diagnostic"
                        } else {
                            "local_exchange_failure"
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
