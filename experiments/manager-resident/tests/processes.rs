use podmesh_manager_ha_lab::{
    durable::{
        inspect_read_only, Configuration as Manager, RefusalReason, Request, Response, Store,
    },
    ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::{ConfigurationFile, Peer};
use podmesh_manager_resident_lab::Configuration;
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

struct Proxy {
    address: SocketAddr,
    blocked: Arc<AtomicBool>,
    /// How long a new connection is held before it is relayed: a peer that
    /// answers, late, rather than one that is unreachable.
    hold_ms: Arc<AtomicU64>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Proxy {
    fn new(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let blocked = Arc::new(AtomicBool::new(false));
        let hold_ms = Arc::new(AtomicU64::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let block = Arc::clone(&blocked);
        let hold = Arc::clone(&hold_ms);
        let worker = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                if let Ok((mut source, _)) = listener.accept() {
                    if block.load(Ordering::SeqCst) {
                        drop(source);
                    } else {
                        let held = hold.load(Ordering::SeqCst);
                        workers.push(thread::spawn(move || {
                            if held > 0 {
                                thread::sleep(Duration::from_millis(held));
                            }
                            if let Ok(mut destination) =
                                TcpStream::connect_timeout(&target, Duration::from_millis(500))
                            {
                                let _ = source.set_read_timeout(Some(Duration::from_secs(3)));
                                let _ = destination.set_read_timeout(Some(Duration::from_secs(3)));
                                let mut source_copy = source.try_clone().unwrap();
                                let mut dest_copy = destination.try_clone().unwrap();
                                let forward = thread::spawn(move || {
                                    let _ = std::io::copy(&mut source_copy, &mut dest_copy);
                                    let _ = dest_copy.shutdown(Shutdown::Write);
                                });
                                let _ = std::io::copy(&mut destination, &mut source);
                                let _ = source.shutdown(Shutdown::Write);
                                let _ = forward.join();
                            }
                        }));
                    }
                }
                workers.retain(|worker| !worker.is_finished());
                thread::sleep(Duration::from_millis(5));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self {
            address,
            blocked,
            hold_ms,
            stopping,
            worker: Some(worker),
        }
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = self.worker.take().unwrap().join();
    }
}

/// A loopback network of one laboratory, `127.b.c.0/24` with one fixed port: the
/// three replicas bind `127.b.c.10`, `.11` and `.12`, the claim holds
/// `127.b.c.1` on that port for as long as the laboratory lives, and each
/// replica's own address stays bound here until that replica is first spawned.
///
/// Reserving ephemeral ports and releasing them before the residents bind them
/// is a race: between the two, the kernel can hand the same port to anything
/// else on the machine, and a resident then refuses to start with `Address
/// already in use`. It is lost rarely, and more often the more laboratories run
/// at once -- in this process, in another suite beside it, or any other program.
/// A network nobody else holds removes the window: a bind of `127.b.c.1` that
/// succeeds is exclusive of every other laboratory, this laboratory's own
/// addresses are never free between its reservation and its first start, and
/// outgoing sockets take their source address from `lo` (`127.0.0.1`), never
/// from these.
struct LoopbackNetwork {
    addresses: Vec<SocketAddr>,
    /// Held until each replica is first spawned, then dropped for it to bind.
    reserved: Vec<Option<TcpListener>>,
    _claim: TcpListener,
}

fn private_loopback_network() -> LoopbackNetwork {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Two laboratories of one process take different candidates; two processes
    // start from different ones.
    let seed = std::process::id()
        .wrapping_mul(2_654_435_761)
        .wrapping_add(NEXT.fetch_add(1, Ordering::SeqCst).wrapping_mul(7_919));
    for attempt in 0..4_096 {
        let candidate = seed.wrapping_add(attempt);
        let b = u8::try_from(candidate / 251 % 251).unwrap() + 2;
        let c = u8::try_from(candidate % 251).unwrap() + 2;
        // Below the ephemeral range, so no outgoing connection is ever given it.
        let port = 10_000 + u16::try_from(candidate % 10_000).unwrap();
        let address = |last: u8| SocketAddr::from((std::net::Ipv4Addr::new(127, b, c, last), port));
        let Ok(claim) = TcpListener::bind(address(1)) else {
            continue;
        };
        // The replicas' addresses are taken here and held until they are spawned:
        // a bind that fails leaves this network to whoever holds it.
        let reserved: Vec<_> = (0..3).map(|i| TcpListener::bind(address(10 + i))).collect();
        if reserved.iter().any(Result::is_err) {
            continue;
        }
        return LoopbackNetwork {
            addresses: (0..3).map(|i| address(10 + i)).collect(),
            reserved: reserved.into_iter().map(Result::ok).collect(),
            _claim: claim,
        };
    }
    panic!("no free private loopback network for this laboratory");
}

struct Lab {
    _dir: tempfile::TempDir,
    _network: LoopbackNetwork,
    configs: Vec<Configuration>,
    children: Vec<Option<Child>>,
}
impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let network = private_loopback_network();
        let addresses = network.addresses.clone();
        let manager = Manager {
            logical_manager_id: "resident-test".into(),
            replicas: (0..3)
                .map(|i| ReplicaConfig {
                    replica_id: format!("r{i}"),
                    host_id: format!("h{i}"),
                })
                .collect(),
            grants: (0..3)
                .map(|i| ScopeGrant {
                    scope: format!("s{i}"),
                    owner_replica_id: format!("r{i}"),
                })
                .collect(),
        };
        let configs = (0..3)
            .map(|i| Configuration {
                network: ConfigurationFile {
                    replica_id: format!("r{i}"),
                    database_path: dir.path().join(format!("r{i}.sqlite")),
                    manager: manager.clone(),
                    bind: addresses[i],
                    peers: (0..3)
                        .filter(|j| *j != i)
                        .map(|j| Peer {
                            replica_id: format!("r{j}"),
                            endpoint: addresses[j],
                            shared_key_hex: format!("{:02x}", i.min(j) * 3 + i.max(j) + 1)
                                .repeat(32),
                        })
                        .collect(),
                },
                control_socket: dir.path().join(format!("r{i}.sock")),
                observation_writer_uid: rustix::process::geteuid().as_raw(),
                interval_ms: 100,
                max_backoff_ms: 400,
                incoming_workers: 2,
                full_verification_interval_ms: None,
                unchanged_snapshot_refresh_ms: None,
                catch_up_window_ms: None,
            })
            .collect();
        Self {
            _dir: dir,
            _network: network,
            configs,
            children: vec![None, None, None],
        }
    }
    fn store(&self, i: usize) -> Store {
        let c = &self.configs[i].network;
        Store::open(&c.database_path, c.manager.clone(), &c.replica_id).unwrap()
    }
    fn observe(&self, i: usize, operation: &str, resource: Option<&str>) {
        self.store(i)
            .execute(&Request::Observe {
                operation_id: operation.into(),
                scope: format!("s{i}"),
                subject: operation.into(),
                exclusive_resource: resource.map(str::to_owned),
                active_claim: resource.is_some(),
                value: "test-observation".into(),
            })
            .unwrap();
    }
    /// Where a replica's standard error is kept. Every start of that replica
    /// appends to it, and a start that fails quotes its last lines: a resident
    /// that exits says why on standard error, and nothing else does.
    fn stderr_log(&self, i: usize) -> std::path::PathBuf {
        self._dir.path().join(format!("r{i}.stderr"))
    }
    fn config_path(&self, i: usize) -> std::path::PathBuf {
        self._dir.path().join(format!("r{i}.json"))
    }
    /// The end of a replica's standard error, for a failure message.
    fn stderr_tail(&self, i: usize) -> String {
        let path = self.stderr_log(i);
        match fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => text
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|line| format!("    | {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            Ok(_) => format!("    | (nothing on standard error in {})", path.display()),
            Err(error) => format!("    | ({}: {error})", path.display()),
        }
    }
    /// What a replica was given, for a failure message: a start that fails on a
    /// path, a socket or an address names the one it was handed.
    fn spawn_description(&self, i: usize) -> String {
        let config = &self.configs[i];
        format!(
            "config {}, store {}, control socket {} (exists: {}), bind {}, peers [{}]",
            self.config_path(i).display(),
            config.network.database_path.display(),
            config.control_socket.display(),
            config.control_socket.exists(),
            config.network.bind,
            config
                .network
                .peers
                .iter()
                .map(|peer| format!("{} at {}", peer.replica_id, peer.endpoint))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
    /// The laboratory's own reservation of a replica's address, to hold or to
    /// bind in place of that replica. Binding that address afresh would race
    /// whatever else runs on this machine; this cannot. The replica must not be
    /// started while it is held.
    fn take_address(&mut self, i: usize) -> TcpListener {
        self._network.reserved[i]
            .take()
            .unwrap_or_else(|| panic!("r{i}'s address was already released by a start"))
    }
    /// Releases a replica's address for a resident this laboratory does not spawn
    /// itself.
    fn release_address(&mut self, i: usize) {
        drop(self._network.reserved[i].take());
    }
    fn start(&mut self, i: usize) {
        // This replica's address was held from the laboratory's first moment so
        // that nothing could take it in between; the resident binds it now.
        self.release_address(i);
        let path = self.config_path(i);
        fs::write(&path, serde_json::to_vec(&self.configs[i]).unwrap()).unwrap();
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.stderr_log(i))
            .unwrap();
        self.children[i] = Some(
            Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
                .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap(),
        );
        let (bound, allowance) = readiness_bound();
        let started = Instant::now();
        loop {
            let exited = self.children[i].as_mut().unwrap().try_wait().unwrap();
            if let Some(exit) = exited {
                panic!(
                    "resident r{i} exited while starting: {exit}\n  it was given {}\n  \
                     the end of its standard error:\n{}",
                    self.spawn_description(i),
                    self.stderr_tail(i)
                );
            }
            if self.control(i, "status").is_some() {
                break;
            }
            assert!(
                started.elapsed() < bound + allowance,
                "resident r{i} did not answer status within {bound:?} (load allowance \
                 {allowance:?})\n  it was given {}\n  the end of its standard error:\n{}",
                self.spawn_description(i),
                self.stderr_tail(i)
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn status(&self, i: usize) -> Value {
        let (bound, allowance) = readiness_bound();
        let start = Instant::now();
        loop {
            if let Some(reply) = self.control(i, "status") {
                if reply["kind"] == "resident_observation" {
                    return reply;
                }
            }
            assert!(
                start.elapsed() < bound + allowance,
                "status of r{i} did not become available within {bound:?} (load allowance \
                 {allowance:?})\n  {}\n  the end of its standard error:\n{}",
                self.resident_state(i),
                self.stderr_tail(i)
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn status_once_within(&self, i: usize, bound: Duration) -> Value {
        let started = Instant::now();
        let reply = self
            .control(i, "status")
            .expect("single status request returned no typed response");
        assert!(
            started.elapsed() < bound,
            "single status request exceeded {bound:?}: {:?}",
            started.elapsed()
        );
        assert_eq!(reply["kind"], "resident_observation");
        reply
    }
    fn control(&self, i: usize, operation: &str) -> Option<Value> {
        self.control_value(i, json!({"operation":operation}))
    }
    fn control_value(&self, i: usize, request: Value) -> Option<Value> {
        self.control_raw(i, &serde_json::to_vec(&request).unwrap())
    }
    fn control_raw(&self, i: usize, request: &[u8]) -> Option<Value> {
        serde_json::from_slice(&self.control_bytes(i, request)?).ok()
    }
    /// What the operating system says of a replica's process, for a failure
    /// message: a control request without an answer means one thing while the
    /// resident runs and another once it has exited.
    fn resident_state(&self, i: usize) -> String {
        let Some(child) = self.children[i].as_ref() else {
            return format!("r{i} has no process");
        };
        let pid = child.id();
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                let state = stat
                    .rsplit(") ")
                    .next()
                    .and_then(|rest| rest.split(' ').next())
                    .unwrap_or("?");
                let meaning = match state {
                    "Z" => " (exited, not reaped)",
                    "R" | "S" | "D" => " (running)",
                    _ => "",
                };
                format!("r{i} is pid {pid} in state {state}{meaning}")
            }
            Err(_) => format!("r{i} is pid {pid}, gone"),
        }
    }
    /// One control request that must be answered. A failure says what the socket
    /// did, what the resident's process is doing and what it last said, rather
    /// than unwrapping nothing.
    #[track_caller]
    fn control_expect(&self, i: usize, request: Value) -> Value {
        let body = serde_json::to_vec(&request).unwrap();
        let bytes = self.control_io(i, &body).unwrap_or_else(|error| {
            panic!(
                "control request {request} to r{i} was not answered: {error}\n  {}\n  \
                 it was given {}\n  the end of its standard error:\n{}",
                self.resident_state(i),
                self.spawn_description(i),
                self.stderr_tail(i)
            )
        });
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "r{i} answered {} to {request}, which is not a typed reply: {error}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }
    fn control_bytes(&self, i: usize, request: &[u8]) -> Option<Vec<u8>> {
        self.control_io(i, request).ok()
    }
    fn control_io(&self, i: usize, request: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut s = UnixStream::connect(&self.configs[i].control_socket)?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.write_all(request)?;
        s.shutdown(Shutdown::Write)?;
        let mut bytes = Vec::new();
        s.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn append(
        &self,
        i: usize,
        operation_id: &str,
        scope: &str,
        subject: &str,
        value: &str,
    ) -> Value {
        self.append_counting_uncertain(i, operation_id, scope, subject, value)
            .0
    }
    /// Appends and says how many answers were `uncertain`: such an answer may
    /// have committed, so the retry that follows it can be answered with the
    /// original receipt, `replayed`. Under load that is the contract working,
    /// not a surprise.
    fn append_counting_uncertain(
        &self,
        i: usize,
        operation_id: &str,
        scope: &str,
        subject: &str,
        value: &str,
    ) -> (Value, usize) {
        let request = json!({
            "operation": "append_observation",
            "operation_id": operation_id,
            "scope": scope,
            "subject": subject,
            "value": value,
        });
        let start = Instant::now();
        let mut uncertain = 0;
        loop {
            let reply = self.control_expect(i, request.clone());
            if reply["response"]["result"] == "observed" {
                return (reply, uncertain);
            }
            assert!(
                matches!(
                    reply["error"].as_str(),
                    Some(
                        "append_observation_busy"
                            | "append_observation_uncertain"
                            | "append_observation_catching_up"
                    )
                ),
                "unexpected append reply: {reply}"
            );
            if reply["error"] == "append_observation_uncertain" {
                uncertain += 1;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "append remained unavailable"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn count(&self, i: usize) -> usize {
        let c = &self.configs[i].network;
        let start = Instant::now();
        loop {
            match inspect_read_only(&c.database_path, &c.manager, &c.replica_id) {
                Ok(inspection) => return inspection.history_count,
                Err(_) => {
                    assert!(
                        start.elapsed() < Duration::from_secs(5),
                        "inspection remained unavailable"
                    );
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
    fn stop(&mut self, i: usize) {
        assert_eq!(
            self.control_expect(i, json!({"operation": "shutdown"}))["shutdown_requested"],
            true
        );
        let mut child = self.children[i].take().unwrap();
        until(
            || child.try_wait().unwrap().is_some(),
            Duration::from_secs(15),
        );
        assert!(child.wait().unwrap().success());
        assert!(!self.configs[i].control_socket.exists());
    }
    fn start_all(&mut self) {
        for i in 0..3 {
            self.start(i);
        }
    }
    fn stop_all(&mut self) {
        for i in 0..3 {
            if self.children[i].is_some() {
                self.stop(i);
            }
        }
    }
    /// Waits until a resident reports that it has caught up with its peers.
    fn wait_caught_up(&self, i: usize) {
        until(
            || self.status(i)["catch_up"]["caught_up"] == true,
            Duration::from_secs(10),
        );
    }
    /// The database and its SQLite sidecars of a stopped replica.
    fn store_files(&self, i: usize) -> Vec<std::path::PathBuf> {
        ["", "-wal", "-shm"]
            .into_iter()
            .map(|suffix| {
                let mut path = self.configs[i].network.database_path.as_os_str().to_owned();
                path.push(suffix);
                std::path::PathBuf::from(path)
            })
            .collect()
    }
    fn delete_store(&self, i: usize) {
        assert!(self.children[i].is_none());
        for path in self.store_files(i) {
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        for child in self.children.iter_mut().flatten() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
/// How long a resident may take to answer status. The universe's 25-second start
/// budget bounds the control socket's bind (catching up is not bounded by it), so
/// a resident must bind that socket within a few seconds. `PODMESH_RESIDENT_TEST_LOAD_ALLOWANCE_MS`
/// adds an explicit allowance for a machine known to be loaded; a failure names
/// the bound and the allowance apart.
fn readiness_bound() -> (Duration, Duration) {
    let allowance = std::env::var("PODMESH_RESIDENT_TEST_LOAD_ALLOWANCE_MS")
        .ok()
        .and_then(|milliseconds| milliseconds.parse().ok())
        .map_or(Duration::ZERO, Duration::from_millis);
    (Duration::from_secs(5), allowance)
}

#[track_caller]
fn until(mut predicate: impl FnMut() -> bool, timeout: Duration) {
    let start = Instant::now();
    while !predicate() {
        assert!(start.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn three_residents_converge_partition_reconnect_restart_and_preserve_conflicts() {
    let mut lab = Lab::new();
    let mut proxies = Vec::new();
    for (source, config) in lab.configs.iter_mut().enumerate() {
        for peer in &mut config.network.peers {
            let isolated = source == 2 || peer.replica_id == "r2";
            let proxy = Proxy::new(peer.endpoint);
            peer.endpoint = proxy.address;
            proxies.push((isolated, proxy));
        }
    }
    // A replica appends only once it has caught up with both peers, so all three
    // run before the first append.
    lab.start_all();
    for i in 0..3 {
        let reply = lab.append(
            i,
            "initial",
            &format!("s{i}"),
            "initial",
            "test-observation",
        );
        assert_eq!(reply["response"]["result"], "observed");
    }
    until(
        || (0..3).all(|i| lab.count(i) == 3),
        Duration::from_secs(15),
    );
    for i in 0..3 {
        let status = lab.status(i);
        assert_eq!(status["activation_authority"], false);
    }
    // Isolate the still-running r2 with accepting-but-dropping TCP proxies.
    // The other two retain direct communication and append independently.
    for (isolated, proxy) in &proxies {
        if *isolated {
            proxy.blocked.store(true, Ordering::SeqCst);
        }
    }
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        lab.append(
            0,
            "during-partition-r0",
            "s0",
            "partition",
            "test-observation"
        )["response"]["result"],
        "observed"
    );
    assert_eq!(
        lab.append(
            1,
            "during-partition-r1",
            "s1",
            "partition",
            "test-observation"
        )["response"]["result"],
        "observed"
    );
    until(
        || lab.count(0) == 5 && lab.count(1) == 5,
        Duration::from_secs(10),
    );
    assert_eq!(lab.count(2), 3);
    until(
        || {
            let status = lab.status(0);
            let peer = &status["peers"]["r2"];
            peer["local_history_len_at_attempt"] == 5
                && peer["outcome"] != "authenticated_import_receipt"
        },
        Duration::from_secs(10),
    );
    assert!(lab.status(0)["peers"]["r2"]["history_count_delta"].is_null());
    assert!(lab.children[2]
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    for (_, proxy) in &proxies {
        proxy.blocked.store(false, Ordering::SeqCst);
    }
    until(|| lab.count(2) == 5, Duration::from_secs(15));
    // Abrupt process death leaves SQLite intact and the private socket stale.
    lab.children[2].as_mut().unwrap().kill().unwrap();
    lab.children[2].take().unwrap().wait().unwrap();
    fs::remove_file(&lab.configs[2].control_socket).unwrap();
    lab.start(2);
    assert_eq!(lab.count(2), 5);
    lab.observe(0, "claim-a", Some("ip:test"));
    lab.observe(1, "claim-b", Some("ip:test"));
    until(
        || (0..3).all(|i| lab.count(i) == 7),
        Duration::from_secs(15),
    );
    for i in 0..3 {
        assert!(inspect_read_only(
            &lab.configs[i].network.database_path,
            &lab.configs[i].network.manager,
            &lab.configs[i].network.replica_id,
        )
        .unwrap()
        .blocked_exclusive_resources
        .contains(&"ip:test".into()));
    }
    for i in 0..3 {
        lab.stop(i);
    }
}

#[test]
fn wrong_key_and_oversized_frames_do_not_import() {
    let mut lab = Lab::new();
    lab.observe(0, "secret-origin", None);
    for peer in &mut lab.configs[0].network.peers {
        peer.shared_key_hex = if peer.replica_id == "r1" {
            "aa".repeat(32)
        } else {
            "bb".repeat(32)
        };
    }
    for i in 0..3 {
        lab.start(i);
    }
    until(
        || {
            let status = lab.status(0);
            status["peers"]["r1"]["failures"].as_u64().unwrap() > 0
                && status["peers"]["r1"]["outcome"] == "unauthenticated_remote_diagnostic"
        },
        Duration::from_secs(10),
    );
    assert_eq!(lab.count(1), 0);
    assert_eq!(lab.count(2), 0);
    assert_eq!(lab.status(0)["peers"]["r1"]["authenticated_successes"], 0);
    // An oversized frame is answered and imports nothing. A connection that
    // arrives while both incoming workers are busy is closed unread and counted
    // as rejected, which resets it: that is admission, tested on its own, and
    // such an attempt is repeated.
    let mut attempts = 0;
    loop {
        attempts += 1;
        let rejected = lab.status(1)["rejected_connections"].as_u64().unwrap();
        let mut stream = TcpStream::connect(lab.configs[1].network.bind).unwrap();
        stream.write_all(&524_289_u32.to_be_bytes()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reply = Vec::new();
        match stream.read_to_end(&mut reply) {
            Ok(_) => {
                // The answer is a framed, unsigned diagnostic: the sender learns
                // that its frame was refused as malformed, it is not left to a
                // closed connection.
                assert!(reply.len() > 4, "oversized frame answered with {reply:?}");
                let announced = u32::from_be_bytes(reply[..4].try_into().unwrap());
                assert_eq!(usize::try_from(announced).unwrap(), reply.len() - 4);
                let diagnostic: Value = serde_json::from_slice(&reply[4..]).unwrap();
                assert_eq!(diagnostic["result"], "diagnostic", "{diagnostic}");
                assert_eq!(diagnostic["server_replica_id"], "r1");
                assert_eq!(diagnostic["category"], "malformed");
                break;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionReset
                    && attempts < 5
                    && lab.status(1)["rejected_connections"].as_u64().unwrap() > rejected =>
            {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("oversized frame attempt {attempts}: {error}"),
        }
    }
    assert_eq!(lab.count(1), 0);
    // Neither the refused import nor the refused push catches r1 up with r0, and
    // an empty store keeps refusing appends.
    until(
        || lab.status(1)["catch_up"]["peers_imported"] == json!(["r2"]),
        Duration::from_secs(10),
    );
    let catch_up = lab.status(1)["catch_up"].clone();
    assert_eq!(catch_up["peers_missing"], json!(["r0"]));
    assert_eq!(catch_up["caught_up"], false);
    assert_eq!(
        lab.control_expect(
            1,
            json!({"operation":"append_observation","operation_id":"refused-peer","scope":"s1","subject":"subject","value":"value"}),
        )["error"],
        "append_observation_catching_up"
    );
    for i in 0..3 {
        lab.stop(i);
    }
}

/// A start that fails says what happened. The harness quotes the resident's exit
/// status, what it was given and the end of its own standard error: a start that
/// lost a race to its address, to its socket path or to its store is otherwise
/// reported as "exited while starting" and nothing else.
#[test]
fn a_resident_that_exits_while_starting_says_why() {
    let mut lab = Lab::new();
    // Something else holds the address this replica is configured to bind: the
    // laboratory's own reservation of it, which a start would otherwise release.
    let occupied = lab.take_address(1);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lab.start(1)));
    std::panic::set_hook(previous);
    let message = *failure.unwrap_err().downcast::<String>().unwrap();
    for expected in [
        "resident r1 exited while starting: exit status: 1",
        "resident refused: Address already in use",
        &format!("bind {}", lab.configs[1].network.bind),
        "control socket",
        "the end of its standard error",
    ] {
        assert!(
            message.contains(expected),
            "{expected:?} missing from {message}"
        );
    }
    // Nothing else was broken: the replica starts once the address is free.
    drop(occupied);
    lab.start(1);
    lab.stop(1);
}

#[test]
fn flood_has_fixed_worker_bound_and_shutdown_drains() {
    let mut lab = Lab::new();
    lab.start(0);
    let sockets: Vec<_> = (0..30)
        .filter_map(|_| TcpStream::connect(lab.configs[0].network.bind).ok())
        .collect();
    until(
        || lab.status(0)["rejected_connections"].as_u64().unwrap() > 0,
        Duration::from_secs(4),
    );
    let status = lab.status(0);
    assert!(status["peak_incoming"].as_u64().unwrap() <= 2);
    assert_eq!(status["incoming_limit"], 2);
    assert_eq!(status["outgoing_limit"], 1);
    let before = Instant::now();
    lab.stop(0);
    assert!(before.elapsed() < Duration::from_secs(15));
    drop(sockets);
}

#[test]
fn invalid_config_and_second_resident_are_refused() {
    let mut lab = Lab::new();
    let mut invalid = lab.configs[0].clone();
    invalid.incoming_workers = 9;
    assert!(invalid.validate().is_err());
    invalid = lab.configs[0].clone();
    invalid.interval_ms = 0;
    assert!(invalid.validate().is_err());
    invalid = lab.configs[0].clone();
    invalid.network.bind.set_port(0);
    assert!(invalid.validate().is_err());
    invalid = lab.configs[0].clone();
    invalid.network.manager.logical_manager_id = "x".repeat(129);
    assert!(invalid.validate().is_err());
    invalid = lab.configs[0].clone();
    invalid.network.manager.replicas[0].host_id = "\u{1}".repeat(128);
    assert!(invalid.validate().is_err());
    let mut value = serde_json::to_value(&lab.configs[0]).unwrap();
    value["arbitrary_command"] = json!("false");
    assert!(serde_json::from_value::<Configuration>(value).is_err());
    let mut value = serde_json::to_value(&lab.configs[0]).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("observation_writer_uid");
    assert!(serde_json::from_value::<Configuration>(value).is_err());
    lab.start(0);
    // Give the second process distinct free bind/socket paths so refusal cannot
    // be explained by either listener collision: only its DB lock is shared.
    let mut duplicate = lab.configs[0].clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    duplicate.network.bind = listener.local_addr().unwrap();
    duplicate.control_socket = lab._dir.path().join("duplicate.sock");
    drop(listener);
    let duplicate_path = lab._dir.path().join("duplicate.json");
    fs::write(&duplicate_path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
        .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
        .arg(duplicate_path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!duplicate.control_socket.exists());
    lab.stop(0);
}

#[test]
fn unknown_control_fields_are_refused_without_shutdown() {
    let mut lab = Lab::new();
    lab.start(0);
    for operation in ["status", "shutdown"] {
        let reply = lab.control_expect(0, json!({"operation":operation,"unexpected":true}));
        assert_eq!(reply["error"], "invalid typed control request");
        assert!(lab.children[0]
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none());
    }
    assert_eq!(lab.status(0)["kind"], "resident_observation");
    lab.stop(0);
}

#[test]
fn append_observation_is_uid_bound_nonexclusive_and_idempotent() {
    let mut lab = Lab::new();
    // An empty store appends only after every peer caught it up.
    lab.start_all();
    let (first, uncertain) =
        lab.append_counting_uncertain(0, "append.1:local", "s0", "subject-1", "value");
    assert_eq!(first["response"]["result"], "observed");
    assert_eq!(first["response"]["fact"]["exclusive_resource"], Value::Null);
    assert_eq!(first["response"]["fact"]["active_claim"], false);
    assert_eq!(first["receipt"]["kind"], "observe");
    // A first append is answered `replayed` only when an uncertain answer, whose
    // append had in fact committed, was retried.
    assert_eq!(first["replayed"], uncertain > 0, "{first}");
    let replay = lab.append(0, "append.1:local", "s0", "subject-1", "value");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], first["receipt"]);
    assert_eq!(lab.count(0), 1);

    for request in [
        json!({"operation":"append_observation","operation_id":"append.1:local","scope":"s0","subject":"subject-1","value":"different"}),
        json!({"operation":"append_observation","operation_id":"other","scope":"s1","subject":"subject-1","value":"value"}),
        json!({"operation":"append_observation","operation_id":"network:reserved","scope":"s0","subject":"subject-1","value":"value"}),
        json!({"operation":"append_observation","operation_id":"bad/slash","scope":"s0","subject":"subject-1","value":"value"}),
        json!({"operation":"append_observation","operation_id":"other","scope":"s0//child","subject":"subject-1","value":"value"}),
        json!({"operation":"append_observation","operation_id":"other","scope":"s0","subject":"subject/invalid","value":"value"}),
        json!({"operation":"append_observation","operation_id":"other","scope":"s0","subject":"subject-1","value":""}),
        json!({"operation":"append_observation","operation_id":"other","scope":"s0","subject":"subject-1","value":"value","exclusive_resource":"ip:test"}),
        json!({"operation":"append_observation","operation_id":"other","scope":"s0","subject":"subject-1","value":"value","active_claim":true}),
    ] {
        let reply = lab.control_expect(0, request);
        assert!(matches!(
            reply["error"].as_str(),
            Some("append_observation_refused" | "invalid typed control request")
        ));
        assert_eq!(lab.count(0), 1);
    }
    assert!(lab.children[0]
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    lab.stop_all();
}

#[test]
fn append_observation_refuses_another_uid_before_store_access_and_stays_live() {
    let mut lab = Lab::new();
    let current = rustix::process::geteuid().as_raw();
    lab.configs[0].observation_writer_uid = if current == u32::MAX {
        current - 1
    } else {
        current + 1
    };
    lab.start(0);
    let reply = lab
        .control_expect(
            0,
            json!({"operation":"append_observation","operation_id":"uid-refusal","scope":"s0","subject":"subject","value":"value"}),
        );
    assert_eq!(reply["error"], "append_observation_refused");
    assert_eq!(lab.count(0), 0);
    assert_eq!(lab.status(0)["kind"], "resident_observation");
    assert!(lab.children[0]
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    lab.stop(0);
}

#[test]
fn append_observation_accepts_any_4096_byte_utf8_value_and_bounds_raw_frames() {
    let mut lab = Lab::new();
    lab.start_all();
    let escaped = "\\".repeat(4096);
    let reply = lab.append(0, "escaped-value", "s0", "subject", &escaped);
    assert_eq!(reply["response"]["result"], "observed");
    assert_eq!(lab.count(0), 1);
    let controls = "\u{1}".repeat(4096);
    let reply = lab.append(0, "control-value", "s0", "subject", &controls);
    assert_eq!(reply["response"]["result"], "observed");
    assert_eq!(reply["response"]["fact"]["value"], controls);
    assert_eq!(lab.count(0), 2);
    for (size, expected) in [
        (32_768, "invalid typed control request"),
        (32_769, "control request bound exceeded"),
    ] {
        let reply = lab.control_raw(0, &vec![b' '; size]).unwrap();
        assert_eq!(reply["error"], expected);
        assert_eq!(lab.count(0), 2);
    }
    assert!(lab.children[0]
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    lab.stop_all();
}

#[test]
fn append_observation_disconnect_replays_after_restart() {
    let mut lab = Lab::new();
    lab.start_all();
    lab.wait_caught_up(0);
    let request = json!({
        "operation":"append_observation",
        "operation_id":"disconnect-replay",
        "scope":"s0",
        "subject":"subject",
        "value":"value",
    });
    let mut stream = UnixStream::connect(&lab.configs[0].control_socket).unwrap();
    stream
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    drop(stream);
    until(|| lab.count(0) == 1, Duration::from_secs(5));
    lab.children[0].as_mut().unwrap().kill().unwrap();
    lab.children[0].take().unwrap().wait().unwrap();
    fs::remove_file(&lab.configs[0].control_socket).unwrap();
    lab.start(0);
    let replay = lab.append(0, "disconnect-replay", "s0", "subject", "value");
    assert_eq!(replay["replayed"], true);
    let changed = lab
        .control_expect(
            0,
            json!({"operation":"append_observation","operation_id":"disconnect-replay","scope":"s0","subject":"subject","value":"changed"}),
        );
    assert_eq!(changed["error"], "append_observation_refused");
    assert_eq!(lab.count(0), 1);
    lab.stop_all();
}

#[test]
fn status_stays_compact_after_more_than_600_audit_events() {
    let mut lab = Lab::new();
    lab.configs[1].interval_ms = 60_000;
    lab.configs[1].max_backoff_ms = 60_000;
    lab.start(1);
    until(
        || {
            let status = lab.status(1);
            status["peers"]["r0"]["failures"].as_u64().unwrap() > 0
                && status["peers"]["r2"]["failures"].as_u64().unwrap() > 0
        },
        Duration::from_secs(8),
    );
    // Status deliberately has no canonical Store read. Inflate the audit table
    // after its initial network attempts to prove response size does not track
    // retained audit history; --inspect-store remains the canonical verifier.
    let script = r#"
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1], timeout=1)
connection.executemany(
  'INSERT INTO exchange_audit_events VALUES (?, ?, ?, ?, ?, NULL, NULL, NULL, 0, NULL, NULL, 0, NULL, NULL, ?, NULL, NULL, NULL, NULL, NULL, NULL, 0, ?, ?)',
  [(f'audit-{n:04}', f'attempt:{n:064x}', f'nonce-{n}', 'outbound', 'outbound_request_prepared', 'incomplete', '{}', '0' * 64) for n in range(600)])
connection.commit()
print(connection.execute('SELECT count(*) FROM exchange_audit_events').fetchone()[0])
"#;
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&lab.configs[1].network.database_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap()
            >= 600
    );
    let bytes = lab.control_bytes(1, br#"{"operation":"status"}"#).unwrap();
    assert!(bytes.len() <= 16_384);
    let status: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status["kind"], "resident_observation");
    assert_eq!(
        status["canonical_inspection_available_via"],
        "--inspect-store"
    );
    assert!(status.get("inspection").is_none());
    lab.children[1].as_mut().unwrap().kill().unwrap();
    lab.children[1].take().unwrap().wait().unwrap();
    let _ = fs::remove_file(&lab.configs[1].control_socket);
}

#[test]
fn append_busy_deadline_keeps_shutdown_responsive_and_retry_recovers_commit() {
    let mut lab = Lab::new();
    lab.configs[0].interval_ms = 60_000;
    lab.configs[0].max_backoff_ms = 60_000;
    // The peers run so that r0 has caught up with them before its store is locked.
    lab.start_all();
    lab.wait_caught_up(0);
    let ready = lab._dir.path().join("sqlite-lock-ready");
    let mut holder = Command::new("python3")
        .arg("-c")
        .arg(
            "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('BEGIN IMMEDIATE'); open(sys.argv[2], 'w').close(); sys.stdin.read(); c.rollback()",
        )
        .arg(&lab.configs[0].network.database_path)
        .arg(&ready)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    until(|| ready.exists(), Duration::from_secs(3));
    let request = json!({
        "operation":"append_observation",
        "operation_id":"busy-deadline",
        "scope":"s0",
        "subject":"subject",
        "value":"value",
    });
    // The resident answers within its 250 ms control deadline; the bound here adds
    // the load allowance, since a loaded machine can delay the client's own wake.
    let answer_bound = Duration::from_millis(300) + readiness_bound().1;
    let started = Instant::now();
    let reply = lab.control_expect(0, request.clone());
    assert_eq!(reply["error"], "append_observation_uncertain");
    assert!(started.elapsed() < answer_bound, "{:?}", started.elapsed());
    let started = Instant::now();
    let retry = lab.control_expect(0, request.clone());
    assert_eq!(retry["error"], "append_observation_busy");
    assert!(started.elapsed() < answer_bound, "{:?}", started.elapsed());
    let second = json!({
        "operation":"append_observation",
        "operation_id":"busy-second-operation",
        "scope":"s0",
        "subject":"subject",
        "value":"second",
    });
    assert_eq!(
        lab.control_expect(0, second.clone())["error"],
        "append_observation_busy"
    );
    let started = Instant::now();
    assert_eq!(
        lab.control_expect(0, json!({"operation": "shutdown"}))["shutdown_requested"],
        true
    );
    assert!(started.elapsed() < answer_bound, "{:?}", started.elapsed());
    holder.stdin.take().unwrap().write_all(b"release").unwrap();
    assert!(holder.wait().unwrap().success());
    let mut child = lab.children[0].take().unwrap();
    until(
        || child.try_wait().unwrap().is_some(),
        Duration::from_secs(15),
    );
    assert!(child.wait().unwrap().success());
    assert_eq!(lab.count(0), 1);
    lab.start(0);
    lab.wait_caught_up(0);
    let replay = lab.control_expect(0, request);
    assert_eq!(replay["replayed"], true);
    let observed = lab.control_expect(0, second);
    assert_eq!(observed["response"]["result"], "observed");
    assert_eq!(lab.count(0), 2);
    lab.stop_all();
}

#[test]
fn status_remains_available_during_stalled_inbound_and_down_peer_attempts() {
    let mut lab = Lab::new();
    // r1 is never started here: this listener answers on its address, and it is
    // the laboratory's own reservation of it.
    assert_eq!(
        lab.configs[0].network.peers[0].endpoint,
        lab.configs[1].network.bind
    );
    let passive = lab.take_address(1);
    let (accepted_tx, accepted_rx) = std::sync::mpsc::sync_channel(1);
    let passive_worker = thread::spawn(move || {
        let (_stream, _) = passive.accept().unwrap();
        accepted_tx.send(()).unwrap();
        thread::sleep(Duration::from_secs(3));
    });
    lab.start(0);
    accepted_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut stream = TcpStream::connect(lab.configs[0].network.bind).unwrap();
    stream.write_all(&100_u32.to_be_bytes()).unwrap();
    until(
        || lab.status(0)["active_incoming"].as_u64().unwrap() > 0,
        Duration::from_secs(2),
    );
    // The outgoing peer withholds its reply while the incoming partial frame
    // holds another network worker. Every single status call must prove that
    // both stalls are still active and remain independently bounded.
    for _ in 0..4 {
        let status = lab.status_once_within(0, Duration::from_millis(300));
        assert_eq!(status["peers"]["r1"]["failures"], 0);
        assert!(status["active_incoming"].as_u64().unwrap() > 0);
        thread::sleep(Duration::from_millis(100));
    }
    drop(stream);
    assert!(lab.children[0]
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    lab.stop(0);
    passive_worker.join().unwrap();
}

#[test]
fn status_is_diagnostic_when_external_inspection_source_is_unavailable() {
    let mut lab = Lab::new();
    lab.configs[0].interval_ms = 60_000;
    lab.configs[0].max_backoff_ms = 60_000;
    lab.start(0);
    let path = &lab.configs[0].network.database_path;
    fs::rename(path, path.with_extension("retained.sqlite")).unwrap();
    fs::create_dir(path).unwrap();
    // Live status deliberately has no Store inspection work; recovery is via the
    // external --inspect-store command once the source is restored.
    assert_eq!(lab.status(0)["kind"], "resident_observation");
    fs::remove_dir(path).unwrap();
    fs::rename(path.with_extension("retained.sqlite"), path).unwrap();
    assert!(lab.children[0]
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert_eq!(lab.status(0)["kind"], "resident_observation");
    lab.stop(0);
}

fn candidate(lab: &Lab, config: &Configuration) -> Command {
    candidate_dirs(lab, config, lab._dir.path(), lab._dir.path())
}

fn candidate_dirs(
    lab: &Lab,
    config: &Configuration,
    state: &std::path::Path,
    runtime: &std::path::Path,
) -> Command {
    let path = lab._dir.path().join("candidate.json");
    fs::write(&path, serde_json::to_vec(config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"));
    command
        .env_remove("PODMESH_MANAGER_NETWORK_MODE")
        .arg("--config")
        .arg(path)
        .arg("--state-dir")
        .arg(state)
        .arg("--runtime-dir")
        .arg(runtime);
    command
}

fn inspect_candidate(lab: &Lab, config: &Configuration) -> Command {
    let path = lab._dir.path().join("inspect.json");
    fs::write(&path, serde_json::to_vec(config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"));
    command
        .env_remove("PODMESH_MANAGER_NETWORK_MODE")
        .arg("--inspect-store")
        .arg("--config")
        .arg(path)
        .arg("--state-dir")
        .arg(lab._dir.path());
    command
}

fn store_source_bytes(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    ["", "-wal", "-shm"]
        .into_iter()
        .filter_map(|suffix| {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(suffix);
            let candidate = std::path::PathBuf::from(candidate);
            candidate
                .exists()
                .then(|| (suffix.into(), fs::read(candidate).unwrap()))
        })
        .collect()
}

#[test]
fn inspect_store_is_network_independent_and_does_not_mutate_source() {
    let lab = Lab::new();
    lab.observe(0, "inspection-source", None);
    let before = store_source_bytes(&lab.configs[0].network.database_path);
    let mut inspection_config = lab.configs[0].clone();
    inspection_config.network.bind = "127.0.0.1:0".parse().unwrap();
    inspection_config.network.peers[0].shared_key_hex = "not-a-key".into();
    inspection_config.interval_ms = 0;
    inspection_config.max_backoff_ms = 0;
    inspection_config.incoming_workers = 0;
    let output = inspect_candidate(&lab, &inspection_config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inspection: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspection["schema_version"], 3);
    assert_eq!(inspection["history_count"], 1);
    assert_eq!(inspection["sqlite_integrity_result"], "ok");
    assert_eq!(
        before,
        store_source_bytes(&lab.configs[0].network.database_path)
    );

    let missing = Lab::new();
    let output = inspect_candidate(&missing, &missing.configs[0])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!missing.configs[0].network.database_path.exists());
}

#[test]
fn hierarchical_owned_scope_uses_the_control_api() {
    let mut lab = Lab::new();
    // Every replica declares the same topology, or they refuse each other's imports.
    for config in &mut lab.configs {
        config.network.manager.grants[0].scope = "s0/local".into();
    }
    lab.start_all();
    let reply = lab.append(0, "hierarchical-scope", "s0/local", "subject", "value");
    assert_eq!(reply["response"]["result"], "observed");
    assert_eq!(lab.count(0), 1);
    lab.stop_all();
}

#[test]
fn candidate_validates_offline_and_network_requires_explicit_opt_in() {
    let lab = Lab::new();
    // Holding the exact configured address makes an accidental validation bind
    // fail; successful validation must remain fully offline.
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut held_config = lab.configs[0].clone();
    held_config.network.bind = occupied.local_addr().unwrap();
    for mode in [None, Some("disabled"), Some("authenticated-static-peers")] {
        let mut command = candidate(&lab, &held_config);
        command.arg("--validate-config");
        if let Some(mode) = mode {
            command.env("PODMESH_MANAGER_NETWORK_MODE", mode);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["durable_store_checked"],
            false
        );
        assert!(!lab.configs[0].network.database_path.exists());
        assert!(!lab.configs[0]
            .network
            .database_path
            .with_extension("resident-lock")
            .exists());
        assert!(!lab.configs[0].control_socket.exists());
    }
    for mode in [None, Some("disabled"), Some("unexpected")] {
        let mut command = candidate(&lab, &lab.configs[0]);
        if let Some(mode) = mode {
            command.env("PODMESH_MANAGER_NETWORK_MODE", mode);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("NETWORK_MODE")
                || String::from_utf8_lossy(&output.stderr).contains("networking disabled")
        );
        assert!(!lab.configs[0].network.database_path.exists());
    }
    let mut wrong_key = lab.configs[0].clone();
    wrong_key.network.peers[0].shared_key_hex = "bad".into();
    assert!(!candidate(&lab, &wrong_key)
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    assert!(!lab.configs[0].network.database_path.exists());
    // Existing arbitrary bytes are not a database-validation target: offline
    // validation checks paths/topology only and must leave these bytes untouched.
    fs::write(
        &lab.configs[0].network.database_path,
        b"not a SQLite database",
    )
    .unwrap();
    assert!(candidate(&lab, &lab.configs[0])
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        fs::read(&lab.configs[0].network.database_path).unwrap(),
        b"not a SQLite database"
    );
}

#[test]
fn candidate_rejects_cli_ambiguity_and_reports_version_without_config() {
    let lab = Lab::new();
    let binary = env!("CARGO_BIN_EXE_podmesh-manager-resident-lab");
    let output = Command::new(binary).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("podmesh-managerd {}\n", env!("CARGO_PKG_VERSION"))
    );
    for flags in [
        vec!["--unknown"],
        vec!["--config"],
        vec!["--version", "--version"],
        vec!["--validate-config"],
        vec!["--state-dir", "--runtime-dir"],
    ] {
        assert!(!Command::new(binary)
            .args(flags)
            .output()
            .unwrap()
            .status
            .success());
    }
    for flags in [
        vec!["--validate-config", "--validate-config"],
        vec!["--config", "/tmp/unused"],
        vec!["--runtime-dir", "/tmp"],
        vec!["--version"],
    ] {
        assert!(!candidate(&lab, &lab.configs[0])
            .args(flags)
            .output()
            .unwrap()
            .status
            .success());
    }
}

#[test]
fn candidate_enforces_direct_owned_nonsymlink_path_boundaries() {
    use std::os::unix::fs::symlink;
    let lab = Lab::new();
    let original = &lab.configs[0];
    for db in [
        std::path::PathBuf::from("relative.sqlite"),
        lab._dir.path().join("sub/../r0.sqlite"),
        lab._dir.path().join("./r0.sqlite"),
        lab._dir.path().join("sub/r0.sqlite"),
        lab._dir.path().parent().unwrap().join("outside.sqlite"),
    ] {
        let mut config = original.clone();
        config.network.database_path = db;
        assert!(!candidate(&lab, &config)
            .arg("--validate-config")
            .output()
            .unwrap()
            .status
            .success());
    }
    let outside = lab._dir.path().join("retained");
    fs::write(&outside, b"keep").unwrap();
    symlink(&outside, &original.network.database_path).unwrap();
    assert!(!candidate(&lab, original)
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    fs::remove_file(&original.network.database_path).unwrap();
    fs::hard_link(&outside, &original.network.database_path).unwrap();
    assert!(!candidate(&lab, original)
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    fs::remove_file(&original.network.database_path).unwrap();
    let alias = lab._dir.path().join("alias");
    symlink(lab._dir.path(), &alias).unwrap();
    let mut config = original.clone();
    config.network.database_path = alias.join("r0.sqlite");
    assert!(!candidate(&lab, &config)
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    assert!(!candidate_dirs(&lab, &config, &alias, lab._dir.path())
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    assert!(!candidate_dirs(
        &lab,
        original,
        std::path::Path::new("relative"),
        lab._dir.path()
    )
    .arg("--validate-config")
    .output()
    .unwrap()
    .status
    .success());
    let mut socket_outside = original.clone();
    socket_outside.control_socket = lab._dir.path().join("sub/control.sock");
    assert!(!candidate(&lab, &socket_outside)
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    fs::set_permissions(lab._dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(!candidate(&lab, original)
        .arg("--validate-config")
        .output()
        .unwrap()
        .status
        .success());
    fs::set_permissions(lab._dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn package_directory_modes_validate_without_creating_state() {
    let lab = Lab::new();
    let state = lab._dir.path().join("state");
    let runtime = lab._dir.path().join("run");
    fs::create_dir(&state).unwrap();
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = lab.configs[0].clone();
    config.network.database_path = state.join("manager.sqlite");
    config.control_socket = runtime.join("control.sock");
    let output = candidate_dirs(&lab, &config, &state, &runtime)
        .arg("--validate-config")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_dir(state).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(!candidate_dirs(
        &lab,
        &config,
        config.network.database_path.parent().unwrap(),
        &runtime
    )
    .arg("--validate-config")
    .output()
    .unwrap()
    .status
    .success());
}

#[test]
fn candidate_flag_mode_runs_and_shuts_down_with_explicit_network_mode() {
    let mut lab = Lab::new();
    // The resident is spawned here rather than by the laboratory, so it releases
    // r0's address itself.
    lab.release_address(0);
    lab.children[0] = Some(
        candidate(&lab, &lab.configs[0])
            .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    until(
        || lab.control(0, "status").is_some(),
        Duration::from_secs(5),
    );
    lab.stop(0);
}

#[test]
fn accepted_transport_connection_has_absolute_trickle_deadline() {
    let lab = Lab::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mut config = lab.configs[1].network.clone();
    config.bind = address;
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        config.open().unwrap().serve_connection(stream)
    });
    let mut socket = TcpStream::connect(address).unwrap();
    socket.write_all(&100_u32.to_be_bytes()).unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(2300) {
        if socket.write_all(b" ").is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = worker.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_eq!(lab.count(1), 0);
}

#[test]
fn authenticated_invalid_batch_commits_no_partial_import() {
    use hmac::{Hmac, Mac};
    use sha2::Digest;
    let mut lab = Lab::new();
    lab.observe(0, "valid-first", None);
    lab.observe(0, "invalid-second", None);
    let Response::Snapshot { mut snapshot } = lab.store(0).execute(&Request::Export {}).unwrap()
    else {
        panic!("snapshot expected");
    };
    snapshot.facts[1].scope = "s1".into();
    let protocol = "podmesh-manager-network-lab/1";
    let op = "atomic-batch";
    let nonce = "atomic-nonce";
    let bytes = serde_json::to_vec(&(protocol, "r0", "r1", op, nonce, &snapshot)).unwrap();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&[2_u8; 32]).unwrap();
    mac.update(b"podmesh-manager-network-lab/1\0");
    mac.update(&bytes);
    let mac_hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let body = serde_json::to_vec(&json!({
        "protocol": protocol,
        "source_replica_id": "r0",
        "destination_replica_id": "r1",
        "operation_id": op,
        "nonce": nonce,
        "snapshot": snapshot,
        "mac_hex": mac_hex,
    }))
    .unwrap();
    let request_sha256 = format!("{:x}", sha2::Sha256::digest(&body));
    lab.start(1);
    let mut stream = TcpStream::connect(lab.configs[1].network.bind).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut reply = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["result"], "refused");
    assert_eq!(reply["source_replica_id"], "r1");
    assert_eq!(reply["destination_replica_id"], "r0");
    assert_eq!(reply["operation_id"], op);
    assert_eq!(reply["nonce"], nonce);
    assert_eq!(reply["request_sha256"], request_sha256);
    assert_eq!(
        serde_json::from_value::<RefusalReason>(reply["reason"].clone()).unwrap(),
        RefusalReason::PolicyViolation
    );
    let signed = serde_json::to_vec(&(
        "refused",
        "r1",
        "r0",
        op,
        nonce,
        request_sha256.as_str(),
        RefusalReason::PolicyViolation,
    ))
    .unwrap();
    let mut expected = Hmac::<sha2::Sha256>::new_from_slice(&[2_u8; 32]).unwrap();
    expected.update(b"podmesh-manager-network-lab/1\0");
    expected.update(&signed);
    let expected_mac: String = expected
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(reply["mac_hex"], expected_mac);
    assert_eq!(lab.count(1), 0);
    lab.stop(1);
}

fn audit_counts(lab: &Lab, replicas: &[usize]) -> Vec<usize> {
    replicas
        .iter()
        .map(|&i| {
            let c = &lab.configs[i].network;
            inspect_read_only(&c.database_path, &c.manager, &c.replica_id)
                .unwrap()
                .audit_event_count
        })
        .collect()
}

/// Waits until no replica's audit table changes for one second.
fn settled_audit_counts(lab: &Lab, replicas: &[usize]) -> Vec<usize> {
    let start = Instant::now();
    let mut counts = audit_counts(lab, replicas);
    let mut unchanged_since = Instant::now();
    while unchanged_since.elapsed() < Duration::from_secs(1) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "audit tables never settled: {counts:?}"
        );
        thread::sleep(Duration::from_millis(100));
        let now = audit_counts(lab, replicas);
        if now != counts {
            counts = now;
            unchanged_since = Instant::now();
        }
    }
    counts
}

fn peer_successes(lab: &Lab, i: usize, peer: &str) -> u64 {
    lab.status(i)["peers"][peer]["authenticated_successes"]
        .as_u64()
        .unwrap()
}

#[test]
fn acknowledged_unchanged_snapshots_are_not_exchanged_every_interval() {
    let mut lab = Lab::new();
    for i in 0..3 {
        lab.start(i);
    }
    lab.append(0, "idle-first", "s0", "idle", "value");
    until(
        || (0..3).all(|i| lab.count(i) == 1),
        Duration::from_secs(10),
    );
    // Once every peer has acknowledged each replica's current snapshot, idle
    // replicas exchange nothing: at a 100 ms interval the previous scheduler
    // added six audit rows per ordered pair every interval.
    let settled = settled_audit_counts(&lab, &[0, 1, 2]);
    let successes = peer_successes(&lab, 0, "r1");
    thread::sleep(Duration::from_millis(1_500));
    assert_eq!(audit_counts(&lab, &[0, 1, 2]), settled);
    assert_eq!(peer_successes(&lab, 0, "r1"), successes);
    assert_eq!(
        lab.status(0)["peers"]["r1"]["outcome"],
        "authenticated_import_receipt"
    );
    // A changed snapshot is still pushed at the next interval.
    lab.append(1, "idle-change", "s1", "idle", "value");
    until(
        || (0..3).all(|i| lab.count(i) == 2),
        Duration::from_secs(10),
    );
    for i in 0..3 {
        lab.stop(i);
    }
}

#[test]
fn acknowledged_unchanged_snapshot_is_refreshed_after_the_configured_delay() {
    let mut lab = Lab::new();
    for config in &mut lab.configs {
        config.unchanged_snapshot_refresh_ms = Some(1_000);
    }
    // All peers run: a down peer's two-second connect budget would space the
    // sequential outgoing worker's visits further apart than the refresh.
    for i in 0..3 {
        lab.start(i);
    }
    until(
        || peer_successes(&lab, 0, "r1") > 0,
        Duration::from_secs(10),
    );
    let first = peer_successes(&lab, 0, "r1");
    let started = Instant::now();
    // About one refresh per second, not one exchange per 100 ms interval: the
    // third exchange after this one cannot come before two refreshes have
    // elapsed, however slow a loaded machine makes each exchange, and it comes
    // well before a refresh twice as long would allow.
    until(
        || peer_successes(&lab, 0, "r1") >= first + 3,
        Duration::from_secs(15),
    );
    let elapsed = started.elapsed();
    assert!(
        (Duration::from_millis(1_900)..Duration::from_secs(6)).contains(&elapsed),
        "three exchanges after {elapsed:?}"
    );
    for i in 0..3 {
        lab.stop(i);
    }
}

/// Edits the oldest audit row of a replica's store with the no-update trigger
/// dropped, in one SQLite transaction, while its resident runs.
fn edit_oldest_audit_row(lab: &Lab, i: usize) {
    let script = r"
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1], timeout=5, isolation_level=None)
connection.executescript('''
BEGIN IMMEDIATE;
DROP TRIGGER exchange_audit_events_no_update;
UPDATE exchange_audit_events SET request_frame_bytes = request_frame_bytes + 1
  WHERE rowid = (SELECT min(rowid) FROM exchange_audit_events);
CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;
COMMIT;
''')
";
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&lab.configs[i].network.database_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn periodic_full_verification_fails_the_store_closed_after_an_old_row_is_edited() {
    let mut lab = Lab::new();
    lab.configs[0].full_verification_interval_ms = Some(1_000);
    // r1 keeps the default ten-minute interval; r2 stays down, so both keep
    // appending failed-attempt audit rows after the edited one. Each store holds
    // a fact of its own origin, so each resident appends once its catch-up window
    // has elapsed although r2 never answers.
    for i in 0..2 {
        lab.configs[i].catch_up_window_ms = Some(1_000);
        lab.observe(i, "seed", None);
    }
    let log = lab.stderr_log(0);
    lab.start(0);
    lab.start(1);
    lab.append(0, "before-edit", "s0", "subject", "value");
    lab.append(1, "before-edit", "s1", "subject", "value");
    until(
        || audit_counts(&lab, &[0, 1]).iter().all(|count| *count >= 8),
        Duration::from_secs(10),
    );
    edit_oldest_audit_row(&lab, 0);
    edit_oldest_audit_row(&lab, 1);
    let edited = Instant::now();

    let request = json!({
        "operation": "append_observation",
        "operation_id": "after-edit",
        "scope": "s0",
        "subject": "subject",
        "value": "value",
    });
    // The periodic pass closes the store. An append that ran past its deadline
    // before that may still be draining, and answers busy meanwhile.
    until(
        || {
            fs::read_to_string(&log)
                .unwrap()
                .contains("resident store verification failed: corrupt")
        },
        Duration::from_secs(10),
    );
    until(
        || lab.control_expect(0, request.clone())["error"] == "append_observation_uncertain",
        Duration::from_secs(10),
    );
    // Failed closed for the rest of the process, while status stays available.
    for _ in 0..5 {
        assert_eq!(
            lab.control_expect(0, request.clone())["error"],
            "append_observation_uncertain"
        );
        thread::sleep(Duration::from_millis(100));
    }
    // The status carries the closed state and its reason: what fails closed with
    // the store, the administration app's origin first, reads it there.
    let status = lab.status(0);
    assert_eq!(status["kind"], "resident_observation");
    assert_eq!(status["store_closed"], true);
    let reason = status["store_closed_reason"].as_str().unwrap();
    assert!(reason.starts_with("corrupt: "), "{reason}");
    // r1 serves the same edit without meeting it: its store is not closed.
    assert_eq!(lab.status(1)["store_closed"], false);
    assert_eq!(lab.status(1)["store_closed_reason"], Value::Null);
    let c = &lab.configs[0].network;
    assert!(matches!(
        inspect_read_only(&c.database_path, &c.manager, &c.replica_id),
        Err(podmesh_manager_ha_lab::durable::DurableError::Corrupt(_))
    ));

    // The same edit on r1 is not read by any of its transactions, so it waits
    // for r1's own complete verification: appends keep succeeding meanwhile.
    thread::sleep(Duration::from_millis(1_000).saturating_sub(edited.elapsed()));
    let reply = lab.append(1, "after-edit", "s1", "subject", "value");
    assert_eq!(reply["response"]["result"], "observed");

    // A new resident process verifies the whole store before serving it.
    lab.stop(0);
    let path = lab._dir.path().join("r0.json");
    let mut restarted = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
        .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    until(
        || restarted.try_wait().unwrap().is_some(),
        Duration::from_secs(10),
    );
    let output = restarted.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("corrupt"));
    assert!(!lab.configs[0].control_socket.exists());
    lab.stop(1);
}

#[test]
fn optional_verification_and_refresh_settings_are_bounded() {
    let lab = Lab::new();
    let mut config = lab.configs[0].clone();
    assert_eq!(
        config.full_verification_interval(),
        podmesh_manager_ha_lab::durable::DEFAULT_FULL_VERIFICATION_INTERVAL
    );
    assert_eq!(
        config.unchanged_snapshot_refresh(),
        podmesh_manager_resident_lab::DEFAULT_UNCHANGED_SNAPSHOT_REFRESH
    );
    let serialized = serde_json::to_value(&config).unwrap();
    assert!(serialized.get("full_verification_interval_ms").is_none());
    assert!(serialized.get("unchanged_snapshot_refresh_ms").is_none());
    for (full, refresh, valid) in [
        (Some(1_000), Some(100), true),
        (Some(86_400_000), Some(3_600_000), true),
        (Some(999), None, false),
        (Some(86_400_001), None, false),
        (None, Some(99), false),
        (None, Some(3_600_001), false),
    ] {
        config.full_verification_interval_ms = full;
        config.unchanged_snapshot_refresh_ms = refresh;
        assert_eq!(config.validate().is_ok(), valid, "{full:?} {refresh:?}");
    }
    config.unchanged_snapshot_refresh_ms = Some(2_500);
    config.full_verification_interval_ms = Some(5_000);
    assert_eq!(
        config.unchanged_snapshot_refresh(),
        Duration::from_millis(2_500)
    );
    assert_eq!(config.full_verification_interval(), Duration::from_secs(5));
    assert_eq!(
        config.catch_up_window(),
        podmesh_manager_resident_lab::DEFAULT_CATCH_UP_WINDOW
    );
    assert!(serialized.get("catch_up_window_ms").is_none());
    for (window, valid) in [
        (Some(1_000), true),
        (Some(15_000), true),
        (Some(999), false),
        (Some(15_001), false),
    ] {
        config.catch_up_window_ms = window;
        assert_eq!(config.validate().is_ok(), valid, "{window:?}");
    }
    config.catch_up_window_ms = Some(3_000);
    assert_eq!(config.catch_up_window(), Duration::from_secs(3));
}

fn peer_mut<'a>(config: &'a mut Configuration, peer: &str) -> &'a mut Peer {
    config
        .network
        .peers
        .iter_mut()
        .find(|candidate| candidate.replica_id == peer)
        .unwrap()
}

fn boot_append(operation: &str) -> Value {
    json!({
        "operation": "append_observation",
        "operation_id": operation,
        "scope": "s2",
        "subject": "boot",
        "value": format!("{operation} value"),
    })
}

/// Repeats one append until it is observed. Returns the observed reply and how
/// many times the resident answered that it was still catching up; once an
/// append has gone past the catch-up gate, the gate never answers again.
fn append_when_caught_up(
    lab: &Lab,
    i: usize,
    request: &Value,
    timeout: Duration,
) -> (Value, usize) {
    let started = Instant::now();
    let mut catching_up = 0;
    let mut past_gate = false;
    loop {
        let reply = lab.control_expect(i, request.clone());
        if reply["response"]["result"] == "observed" {
            return (reply, catching_up);
        }
        match reply["error"].as_str() {
            Some("append_observation_catching_up") => {
                assert!(!past_gate, "catching up again: {reply}");
                catching_up += 1;
            }
            Some("append_observation_busy" | "append_observation_uncertain") => past_gate = true,
            _ => panic!("unexpected append reply: {reply}"),
        }
        assert!(
            started.elapsed() < timeout,
            "no append observed within {timeout:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn inspection(lab: &Lab, i: usize) -> podmesh_manager_ha_lab::durable::CanonicalStoreInspection {
    let c = &lab.configs[i].network;
    let start = Instant::now();
    loop {
        match inspect_read_only(&c.database_path, &c.manager, &c.replica_id) {
            Ok(inspection) => return inspection,
            Err(error) => {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "inspection remained unavailable: {error}"
                );
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

/// Waits until the three replicas hold `facts` facts and every link's last
/// exchange succeeded, then requires identical canonical histories without a
/// conflict: no replica reused a sequence number under other bytes.
fn assert_converged(lab: &Lab, facts: usize) {
    until(
        || (0..3).all(|i| lab.count(i) == facts),
        Duration::from_secs(15),
    );
    until(
        || {
            (0..3).all(|i| {
                lab.status(i)["peers"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|link| link["outcome"] == "authenticated_import_receipt")
            })
        },
        Duration::from_secs(15),
    );
    let inspections: Vec<_> = (0..3).map(|i| inspection(lab, i)).collect();
    for inspection in &inspections {
        assert_eq!(inspection.history_count, facts);
        assert!(inspection.conflicts.is_empty());
        assert_eq!(
            inspection.logical_history_sha256,
            inspections[0].logical_history_sha256
        );
    }
}

#[test]
fn an_emptied_replica_imports_its_own_facts_before_it_appends_and_takes_the_next_sequence() {
    let mut lab = Lab::new();
    // r1 and r2 reach each other only through proxies that can drop every connection.
    let to_r2 = Proxy::new(lab.configs[2].network.bind);
    let to_r1 = Proxy::new(lab.configs[1].network.bind);
    peer_mut(&mut lab.configs[1], "r2").endpoint = to_r2.address;
    peer_mut(&mut lab.configs[2], "r1").endpoint = to_r1.address;
    lab.start_all();
    for n in 1..=3 {
        let request = boot_append(&format!("boot-{n}"));
        assert_eq!(
            append_when_caught_up(&lab, 2, &request, Duration::from_secs(10)).0["response"]["fact"]
                ["producer_sequence"],
            n
        );
    }
    lab.append(0, "peer-fact", "s0", "peer", "value");
    assert_converged(&lab, 4);

    // r2 loses its store and restarts while r1 can neither reach it nor be reached.
    lab.stop(2);
    lab.delete_store(2);
    to_r1.blocked.store(true, Ordering::SeqCst);
    to_r2.blocked.store(true, Ordering::SeqCst);
    lab.start(2);
    // r0 gives r2 back its three facts; a store that held none of its own at start
    // still appends only once every peer caught it up, and never touches the store
    // before that.
    until(
        || lab.status(2)["catch_up"]["peers_imported"] == json!(["r0"]),
        Duration::from_secs(10),
    );
    assert_eq!(lab.count(2), 4);
    let refused_until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < refused_until {
        assert_eq!(
            lab.control_expect(2, boot_append("boot-4"))["error"],
            "append_observation_catching_up"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let status = lab.status(2);
    assert_eq!(status["catch_up"]["caught_up"], false);
    assert_eq!(status["catch_up"]["caught_up_by"], Value::Null);
    assert_eq!(status["catch_up"]["peers_missing"], json!(["r1"]));
    assert_eq!(status["catch_up"]["own_facts_at_start"], 0);
    assert_eq!(lab.count(2), 4);

    // Once r1 answers again, r2 catches up with it, and its next fact takes the
    // sequence after the three its peers held.
    to_r1.blocked.store(false, Ordering::SeqCst);
    to_r2.blocked.store(false, Ordering::SeqCst);
    let (reply, _) =
        append_when_caught_up(&lab, 2, &boot_append("boot-4"), Duration::from_secs(10));
    let fact = &reply["response"]["fact"];
    assert_eq!(fact["event_id"], "r2:00000000000000000004");
    assert_eq!(fact["subject_revision"], 4);
    assert_eq!(fact["predecessor"], "r2:00000000000000000003");
    let status = lab.status(2);
    assert_eq!(status["catch_up"]["caught_up_by"], "every_peer");
    assert_eq!(status["catch_up"]["peers_missing"], json!([]));
    assert_converged(&lab, 5);

    // With every link open, the same loss is caught up within a few exchanges.
    lab.stop(2);
    lab.delete_store(2);
    let spawned = Instant::now();
    lab.start(2);
    let (reply, catching_up) =
        append_when_caught_up(&lab, 2, &boot_append("boot-5"), Duration::from_secs(10));
    let observed_after = spawned.elapsed();
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000005"
    );
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "every_peer");
    assert_eq!(catch_up["own_facts_at_start"], 0);
    assert_converged(&lab, 6);
    println!(
        "MEASURED emptied store, both peers live: caught up {} ms after the resident started exchanging; \
         its first append was observed {} ms after the process was spawned, after {catching_up} \
         append(s) answered catching_up",
        catch_up["caught_up_after_ms"],
        observed_after.as_millis()
    );
    lab.stop_all();
}

/// Answers catching up, typed, to every append for a while.
#[track_caller]
fn assert_catching_up_for(lab: &Lab, i: usize, operation: &str, duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        assert_eq!(
            lab.control_expect(i, boot_append(operation))["error"],
            "append_observation_catching_up"
        );
        thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn with_a_peer_down_only_a_store_whose_latest_own_fact_it_appended_appends_after_the_window() {
    let mut lab = Lab::new();
    for config in &mut lab.configs {
        config.catch_up_window_ms = Some(2_000);
    }
    lab.start_all();
    for n in 1..=2 {
        lab.append(
            2,
            &format!("boot-{n}"),
            "s2",
            "boot",
            &format!("boot-{n} value"),
        );
    }
    assert_converged(&lab, 2);
    lab.stop(1);

    // An emptied store keeps refusing, typed, past its window while r1 is down,
    // although r0 gave it back both of its facts.
    lab.stop(2);
    lab.delete_store(2);
    lab.start(2);
    until(|| lab.count(2) == 2, Duration::from_secs(10));
    assert_catching_up_for(&lab, 2, "boot-3", Duration::from_secs(3));
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up"], false);
    assert_eq!(catch_up["peers_imported"], json!(["r0"]));
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));
    assert_eq!(catch_up["own_facts_at_start"], 0);
    assert_eq!(catch_up["window_ms"], 2_000);
    assert_eq!(lab.count(2), 2);

    // Restarted, the store holds its two facts, imported rather than appended:
    // this process too waits for r1.
    lab.stop(2);
    lab.start(2);
    assert_catching_up_for(&lab, 2, "boot-3", Duration::from_secs(3));
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["own_facts_at_start"], 2);
    assert_eq!(catch_up["latest_own_fact_appended_locally"], false);
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));

    // r1 returns: r2 catches up with both peers and appends its next fact itself.
    lab.start(1);
    let (reply, _) =
        append_when_caught_up(&lab, 2, &boot_append("boot-3"), Duration::from_secs(10));
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000003"
    );
    assert_converged(&lab, 3);

    // With r1 down again, the store whose latest own fact it appended itself
    // appends once the window has elapsed, having reached r0 and tried r1.
    lab.stop(1);
    lab.stop(2);
    let spawned = Instant::now();
    lab.start(2);
    let (reply, catching_up) =
        append_when_caught_up(&lab, 2, &boot_append("boot-4"), Duration::from_secs(10));
    assert!(spawned.elapsed() >= Duration::from_secs(2));
    assert!(catching_up > 0);
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000004"
    );
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "window");
    assert!(catch_up["caught_up_after_ms"].as_u64().unwrap() >= 2_000);
    println!(
        "MEASURED window with one peer down (window 2,000 ms, the attempt to the stopped peer \
         costing its 2 s connect deadline): caught up {} ms after the resident started exchanging, \
         its boot fact observed {} ms after the process was spawned, after {catching_up} \
         answer(s) of catching up",
        catch_up["caught_up_after_ms"],
        spawned.elapsed().as_millis()
    );
    assert_eq!(catch_up["own_facts_at_start"], 3);
    assert_eq!(catch_up["latest_own_fact_appended_locally"], true);
    assert_eq!(catch_up["peers_matched"], json!(["r0"]));
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));
    assert_eq!(catch_up["peers_not_attempted"], json!([]));

    // r1 comes back and nothing collides.
    lab.start(1);
    assert_converged(&lab, 4);
    lab.stop_all();
}

#[test]
fn an_emptied_store_that_imported_its_own_facts_back_waits_for_every_peer_at_its_next_start() {
    let mut lab = Lab::new();
    for config in &mut lab.configs {
        config.catch_up_window_ms = Some(1_000);
    }
    lab.start_all();
    lab.append(2, "boot-1", "s2", "boot", "boot-1 value");
    assert_converged(&lab, 1);
    // r0 goes down holding r2:1 only; r1 receives r2:2 and r2:3.
    lab.stop(0);
    lab.append(2, "boot-2", "s2", "boot", "boot-2 value");
    lab.append(2, "boot-3", "s2", "boot", "boot-3 value");
    until(|| lab.count(1) == 3, Duration::from_secs(10));
    // r1, the only other holder of r2:2 and r2:3, goes down, and r2 loses its store.
    lab.stop(1);
    lab.stop(2);
    lab.delete_store(2);
    lab.start(0);
    lab.start(2);
    // The first start gets r2:1 back from r0 and refuses while r1 is down, until
    // the universe entrypoint ends it at its budget.
    until(|| lab.count(2) == 1, Duration::from_secs(10));
    assert_catching_up_for(&lab, 2, "boot-after-loss", Duration::from_millis(1_500));
    lab.stop(2);
    // The second start holds r2:1, of its own origin but imported: it does not
    // take r2:2, which r1 holds with other bytes, when its window elapses.
    lab.start(2);
    assert_catching_up_for(&lab, 2, "boot-after-loss", Duration::from_secs(3));
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up"], false);
    assert_eq!(catch_up["own_facts_at_start"], 1);
    assert_eq!(catch_up["latest_own_fact_appended_locally"], false);
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));
    // r1 returns: r2 gets r2:2 and r2:3 back, and its next fact takes the fourth
    // sequence; every replica converges on one history.
    lab.start(1);
    let (reply, _) = append_when_caught_up(
        &lab,
        2,
        &boot_append("boot-after-loss"),
        Duration::from_secs(15),
    );
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000004"
    );
    assert_converged(&lab, 4);
    lab.stop_all();
}

#[test]
fn a_short_window_waits_until_every_peer_was_tried_and_one_caught_up() {
    let mut lab = Lab::new();
    lab.start_all();
    lab.append(2, "boot-1", "s2", "boot", "boot-1 value");
    assert_converged(&lab, 1);
    lab.stop(2);
    let saved: Vec<_> = lab
        .store_files(2)
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).ok();
            (path, bytes)
        })
        .collect();
    lab.start(2);
    lab.append(2, "boot-2", "s2", "boot", "boot-2 value");
    assert_converged(&lab, 2);
    // r1's resident stops, so an attempt to it lasts its 2 s connect deadline;
    // r0, which holds r2:2, stays up.
    lab.stop(1);
    lab.stop(2);
    for (path, bytes) in &saved {
        match bytes {
            Some(bytes) => fs::write(path, bytes).unwrap(),
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }
    assert_eq!(lab.count(2), 1);
    // r2 restarts on its older copy with a 1 s window, r1 listed first.
    lab.configs[2].catch_up_window_ms = Some(1_000);
    lab.configs[2].network.peers.swap(0, 1);
    assert_eq!(lab.configs[2].network.peers[0].replica_id, "r1");
    lab.start(2);
    let (reply, _) = append_when_caught_up(
        &lab,
        2,
        &boot_append("boot-restored"),
        Duration::from_secs(15),
    );
    // It did not append at the 1 s mark: it first reached r0, which pushed r2:2
    // back, then took the next sequence.
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000003"
    );
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "window");
    assert!(catch_up["caught_up_after_ms"].as_u64().unwrap() >= 2_000);
    assert_eq!(catch_up["peers_imported"], json!(["r0"]));
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));
    assert_eq!(catch_up["peers_ahead"], json!([]));
    assert_eq!(catch_up["latest_own_fact_appended_locally"], true);
    until(|| lab.count(0) == 3, Duration::from_secs(10));
    lab.start(1);
    assert_converged(&lab, 3);
    lab.stop_all();
}

fn push_backs(lab: &Lab, i: usize, peer: &str) -> u64 {
    lab.status(i)["peers"][peer]["push_backs"].as_u64().unwrap()
}

#[test]
fn a_replica_restored_from_an_older_store_is_pushed_back_to_within_a_few_intervals() {
    let mut lab = Lab::new();
    for config in &mut lab.configs {
        config.unchanged_snapshot_refresh_ms = Some(600_000);
    }
    lab.start_all();
    lab.append(0, "first", "s0", "subject", "first");
    assert_converged(&lab, 1);
    lab.stop(2);
    let saved: Vec<_> = lab
        .store_files(2)
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).ok();
            (path, bytes)
        })
        .collect();
    lab.start(2);
    lab.append(0, "second", "s0", "subject", "second");
    lab.append(1, "third", "s1", "subject", "third");
    assert_converged(&lab, 3);
    // Every link now holds an acknowledgement of the current snapshot, which
    // nobody pushes again before ten minutes, and no exchange is under way.
    let all_acknowledged = || {
        (0..3).all(|i| {
            lab.status(i)["peers"]
                .as_object()
                .unwrap()
                .values()
                .all(|link| link["acknowledged_unchanged"] == true)
        })
    };
    until(all_acknowledged, Duration::from_secs(15));
    settled_audit_counts(&lab, &[0, 1, 2]);
    assert!(all_acknowledged());
    let before = push_backs(&lab, 0, "r2") + push_backs(&lab, 1, "r2");

    // r2 comes back on its older copy, which lacks two facts its peers believe
    // it acknowledged.
    lab.stop(2);
    for (path, bytes) in &saved {
        match bytes {
            Some(bytes) => fs::write(path, bytes).unwrap(),
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }
    assert_eq!(lab.count(2), 1);
    lab.start(2);
    let bound = Instant::now();
    until(|| lab.count(2) == 3, Duration::from_secs(10));
    let converged = bound.elapsed();
    // A push-back requested while an attempt is already past its due check is
    // served by that attempt, which leaves its acknowledgement unrecorded, and
    // counted at the next one: wait for the counter rather than read it once.
    until(
        || push_backs(&lab, 0, "r2") + push_backs(&lab, 1, "r2") > before,
        Duration::from_secs(5),
    );
    assert!(
        push_backs(&lab, 0, "r2") + push_backs(&lab, 1, "r2") > before,
        "convergence did not come from a push-back: before {before}, r0 {} r1 {}",
        lab.status(0)["peers"]["r2"],
        lab.status(1)["peers"]["r2"],
    );
    // Interval 100 ms: a few intervals, not the ten-minute refresh.
    assert!(converged < Duration::from_secs(3), "{converged:?}");
    assert_converged(&lab, 3);
    println!(
        "MEASURED push-back: r2 restored from an older store held all three facts {} ms after \
         its control socket answered (interval 100 ms, refresh 600 s)",
        converged.as_millis()
    );
    lab.stop_all();
}

#[test]
fn status_reports_catch_up_and_what_the_health_of_a_link_needs() {
    let mut lab = Lab::new();
    lab.start_all();
    lab.append(0, "status-fact", "s0", "subject", "value");
    assert_converged(&lab, 1);
    until(
        || {
            lab.status(0)["peers"]
                .as_object()
                .unwrap()
                .values()
                .all(|link| link["acknowledged_unchanged"] == true)
        },
        Duration::from_secs(15),
    );
    let status = lab.status(0);
    let catch_up = &status["catch_up"];
    assert_eq!(catch_up["caught_up"], true);
    assert_eq!(catch_up["caught_up_by"], "every_peer");
    assert!(catch_up["caught_up_after_ms"].is_u64());
    assert_eq!(catch_up["own_facts_at_start"], 0);
    assert_eq!(catch_up["latest_own_fact_appended_locally"], false);
    assert_eq!(catch_up["window_ms"], 15_000);
    assert_eq!(catch_up["peers_missing"], json!([]));
    assert_eq!(catch_up["peers_ahead"], json!([]));
    assert_eq!(catch_up["peers_not_attempted"], json!([]));
    let refresh =
        u64::try_from(podmesh_manager_resident_lab::DEFAULT_UNCHANGED_SNAPSHOT_REFRESH.as_millis())
            .unwrap();
    for peer in ["r1", "r2"] {
        let link = &status["peers"][peer];
        assert_eq!(link["outcome"], "authenticated_import_receipt");
        assert_eq!(link["refresh_ms"], refresh);
        assert_eq!(link["max_backoff_ms"], 400);
        assert_eq!(link["push_backs"], 0);
        assert!(link["last_success_age_ms"].is_u64());
        assert_eq!(link["last_attempt_age_ms"], link["last_success_age_ms"]);
        assert_eq!(link["authenticated_refusals"], 0);
        assert_eq!(link["last_refusal_reason"], Value::Null);
        assert_eq!(link["refused_imports"], 0);
        assert_eq!(
            link["identity_collisions"],
            json!({"imports_refused": 0, "pushes_refused": 0, "event_id": null})
        );
    }

    // r1 stops answering: the next push to it fails after its last success.
    lab.stop(1);
    lab.append(0, "status-change", "s0", "subject", "changed");
    until(
        || lab.status(0)["peers"]["r1"]["outcome"] != "authenticated_import_receipt",
        Duration::from_secs(10),
    );
    let link = lab.status(0)["peers"]["r1"].clone();
    assert_eq!(link["acknowledged_unchanged"], false);
    assert!(
        link["last_attempt_age_ms"].as_u64().unwrap()
            < link["last_success_age_ms"].as_u64().unwrap()
    );
    assert!(link["next_attempt_in_ms"].as_u64().unwrap() <= 400);
    lab.stop_all();
}

#[test]
fn an_idle_replica_adds_at_most_twelve_audit_rows_per_refresh() {
    let mut lab = Lab::new();
    // A refresh of one second stands for the ten-minute default: an idle
    // replica's audit growth is proportional to it.
    for config in &mut lab.configs {
        config.unchanged_snapshot_refresh_ms = Some(1_000);
    }
    lab.start_all();
    lab.append(0, "idle-growth", "s0", "subject", "value");
    assert_converged(&lab, 1);
    thread::sleep(Duration::from_secs(2));
    let before = audit_counts(&lab, &[0, 1, 2]);
    let started = Instant::now();
    thread::sleep(Duration::from_secs(8));
    let after = audit_counts(&lab, &[0, 1, 2]);
    let refreshes = started.elapsed().as_secs_f64();
    let per_refresh: Vec<f64> = before
        .iter()
        .zip(&after)
        .map(|(before, after)| (after - before) as f64 / refreshes)
        .collect();
    println!(
        "MEASURED idle growth: {per_refresh:.1?} audit rows per replica per second at a \
         one-second refresh (at most two pushes sent at two rows and two received at four \
         per refresh; a visit waits up to one interval past the refresh)"
    );
    // Push-back never turns an idle manager into a busy one.
    assert!(
        per_refresh.iter().all(|rows| *rows <= 13.0),
        "{per_refresh:?}"
    );
    lab.stop_all();
}

#[test]
fn a_periodic_verification_that_could_not_run_is_retried_at_the_exchange_interval() {
    let mut lab = Lab::new();
    lab.configs[0].full_verification_interval_ms = Some(8_000);
    let log = lab.stderr_log(0);
    let spawned = Instant::now();
    lab.start(0);
    lab.start(1);
    lab.start(2);
    lab.append(0, "before-edit", "s0", "subject", "value");
    assert_converged(&lab, 1);
    until(
        || {
            lab.status(0)["peers"]
                .as_object()
                .unwrap()
                .values()
                .all(|link| link["acknowledged_unchanged"] == true)
        },
        Duration::from_secs(10),
    );
    // No transaction reads the oldest audit row: only a complete pass notices it.
    edit_oldest_audit_row(&lab, 0);
    // r0's first periodic pass, eight seconds after it started, cannot open its
    // store: a directory stands at its path. Removing the file's permissions would
    // not do: while one connection of the process holds the file, SQLite keeps
    // the descriptor of a closed one and reuses it for the next open.
    assert!(
        spawned.elapsed() < Duration::from_secs(6),
        "setup outlasted the first pass"
    );
    let database = lab.configs[0].network.database_path.clone();
    let retained = database.with_extension("retained");
    fs::rename(&database, &retained).unwrap();
    fs::create_dir(&database).unwrap();
    let logged = |line: &str| fs::read_to_string(&log).unwrap().contains(line);
    until(
        || logged("resident store verification could not run"),
        Duration::from_secs(10),
    );
    thread::sleep(Duration::from_secs(1));
    fs::remove_dir(&database).unwrap();
    fs::rename(&retained, &database).unwrap();
    let readable = Instant::now();
    until(
        || logged("resident store verification failed: corrupt"),
        Duration::from_secs(10),
    );
    let detected = readable.elapsed();
    // The pass was retried within the maximum backoff, not eight seconds after
    // the pass that could not run.
    assert!(detected < Duration::from_secs(3), "{detected:?}");
    let not_run = fs::read_to_string(&log)
        .unwrap()
        .matches("resident store verification could not run")
        .count();
    assert!(not_run >= 2, "{not_run}");
    assert_eq!(
        lab.control_expect(
            0,
            json!({"operation":"append_observation","operation_id":"after-edit","scope":"s0","subject":"subject","value":"value"}),
        )["error"],
        "append_observation_uncertain"
    );
    println!(
        "MEASURED periodic pass retry: {not_run} passes could not run; the edited row was \
         detected {} ms after the store became readable again (interval 100 ms, maximum \
         backoff 400 ms, verification interval 8 s)",
        detected.as_millis()
    );
    lab.stop_all();
}

#[test]
fn a_replica_without_peers_is_caught_up_at_once() {
    let mut lab = Lab::new();
    let config = &mut lab.configs[0];
    config.network.manager.replicas.truncate(1);
    config.network.manager.grants.truncate(1);
    config.network.peers.clear();
    lab.start(0);
    let reply = lab
        .control_expect(
            0,
            json!({"operation":"append_observation","operation_id":"alone","scope":"s0","subject":"subject","value":"value"}),
        );
    assert_eq!(reply["response"]["result"], "observed", "{reply}");
    let catch_up = lab.status(0)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "no_peers");
    assert_eq!(catch_up["caught_up_after_ms"], 0);
    assert_eq!(catch_up["peers_missing"], json!([]));
    lab.stop(0);
}

/// Changes the type of a column of the oldest audit row, with the no-update
/// trigger dropped: a complete verification cannot read that row back, a
/// storage failure rather than a checksum mismatch.
fn tamper_oldest_audit_row_type(lab: &Lab, i: usize) {
    let script = r"
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1], timeout=5, isolation_level=None)
connection.executescript('''
BEGIN IMMEDIATE;
DROP TRIGGER exchange_audit_events_no_update;
UPDATE exchange_audit_events SET request_frame_bytes = 'not-a-number'
  WHERE rowid = (SELECT min(rowid) FROM exchange_audit_events);
CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;
COMMIT;
''')
";
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&lab.configs[i].network.database_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_store_closed_by_a_storage_failure_is_reported_closed_and_not_retried() {
    let mut lab = Lab::new();
    lab.configs[0].full_verification_interval_ms = Some(1_000);
    let log = lab.stderr_log(0);
    lab.start(0);
    lab.start(1);
    lab.start(2);
    lab.append(0, "before-tamper", "s0", "subject", "value");
    assert_converged(&lab, 1);
    tamper_oldest_audit_row_type(&lab, 0);
    let lines = |line: &str| fs::read_to_string(&log).unwrap().matches(line).count();
    until(
        || lines("resident store verification failed: storage") >= 1,
        Duration::from_secs(10),
    );
    assert_eq!(lines("the store is closed for this process"), 1);
    // The store logs its closure where it records it, naming the file.
    let closure = format!(
        "manager store {} is closed for this process: storage",
        lab.configs[0].network.database_path.display()
    );
    assert_eq!(lines(&closure), 1);
    let not_run = lines("resident store verification could not run");
    thread::sleep(Duration::from_millis(3_500));
    // Later passes keep the one-second interval and say the store is still
    // closed: none is retried as a pass that could not run.
    assert_eq!(lines("resident store verification could not run"), not_run);
    let still_closed = lines("resident store is still closed for this process");
    assert!((2..=5).contains(&still_closed), "{still_closed}");
    assert_eq!(
        lab.control_expect(
            0,
            json!({"operation":"append_observation","operation_id":"after-tamper","scope":"s0","subject":"subject","value":"value"}),
        )["error"],
        "append_observation_uncertain"
    );
    let status = lab.status(0);
    assert_eq!(status["store_closed"], true);
    let reason = status["store_closed_reason"].as_str().unwrap();
    assert!(reason.starts_with("storage: "), "{reason}");
    lab.stop_all();
}

#[test]
fn an_identity_collision_is_counted_on_both_sides_and_named_once() {
    let mut lab = Lab::new();
    // Refused exchanges are retried at the backoff and refreshed every second,
    // so both sides see many refusals.
    for config in &mut lab.configs {
        config.unchanged_snapshot_refresh_ms = Some(1_000);
    }
    let logs: Vec<_> = (0..3).map(|i| lab.stderr_log(i)).collect();
    for i in 0..3 {
        lab.start(i);
    }
    lab.append(2, "boot-1", "s2", "boot", "boot-1 value");
    assert_converged(&lab, 1);
    // The fork the catch-up rule prevents, made directly in the store: r2 loses
    // its store and appends another first fact before its resident runs again.
    lab.stop(2);
    lab.delete_store(2);
    lab.observe(2, "forked", None);
    lab.start(2);
    let collisions =
        |i: usize, peer: &str| lab.status(i)["peers"][peer]["identity_collisions"].clone();
    until(
        || {
            [(0, "r2"), (1, "r2"), (2, "r0"), (2, "r1")]
                .into_iter()
                .all(|(i, peer)| collisions(i, peer)["imports_refused"].as_u64().unwrap() >= 2)
                && collisions(2, "r0")["pushes_refused"].as_u64().unwrap() >= 1
                && collisions(0, "r2")["pushes_refused"].as_u64().unwrap() >= 1
        },
        Duration::from_secs(20),
    );
    thread::sleep(Duration::from_secs(2));
    for (i, peer) in [(0, "r2"), (1, "r2"), (2, "r0"), (2, "r1")] {
        let link = lab.status(i)["peers"][peer].clone();
        assert_eq!(
            link["identity_collisions"]["event_id"],
            "r2:00000000000000000001"
        );
        assert_eq!(
            link["outcome"], "authenticated_remote_refusal",
            "r{i} to {peer}"
        );
        assert_eq!(link["last_refusal_reason"], "policy_violation");
        assert!(
            link["refused_imports"].as_u64().unwrap()
                >= link["identity_collisions"]["imports_refused"]
                    .as_u64()
                    .unwrap()
        );
    }
    // One line on standard error per peer names the collision, however many
    // refusals repeat it.
    let named = |i: usize, peer: &str| {
        fs::read_to_string(&logs[i])
            .unwrap()
            .lines()
            .filter(|line| {
                line.contains(&format!("refused an import from {peer}: event identity collision on r2:00000000000000000001"))
            })
            .count()
    };
    assert_eq!(named(0, "r2"), 1);
    assert_eq!(named(1, "r2"), 1);
    assert_eq!(named(2, "r0"), 1);
    assert_eq!(named(2, "r1"), 1);
    lab.stop_all();
}

/// The files of a stopped replica's store and their bytes, to put back later.
type SavedStore = Vec<(std::path::PathBuf, Option<Vec<u8>>)>;

fn save_store(lab: &Lab, i: usize) -> SavedStore {
    lab.store_files(i)
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).ok();
            (path, bytes)
        })
        .collect()
}

fn restore_store(saved: &SavedStore) {
    for (path, bytes) in saved {
        match bytes {
            Some(bytes) => fs::write(path, bytes).unwrap(),
            None => {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// The value one replica holds under an event ID, read without its resident.
fn fact_value(lab: &Lab, i: usize, event_id: &str) -> Option<String> {
    inspection(lab, i)
        .ordered_facts
        .iter()
        .find(|fact| fact.event_id == event_id)
        .map(|fact| fact.value.clone())
}

/// Leaves the laboratory in the state both window tests start from: r2 restored
/// from an older copy of its store, whose latest own fact it appended itself,
/// and r2:2, the fact that copy lacks, held by r0 alone. Every resident is
/// stopped; r0 holds r2:1 and r2:2, r1 and r2 hold r2:1.
fn older_copy_with_one_holder(lab: &mut Lab) {
    lab.start_all();
    lab.append(2, "boot-1", "s2", "boot", "boot-1 value");
    assert_converged(lab, 1);
    lab.stop(2);
    let saved = save_store(lab, 2);
    lab.stop(1);
    lab.configs[2].catch_up_window_ms = Some(1_000);
    lab.start(2);
    let (reply, _) = append_when_caught_up(lab, 2, &boot_append("boot-2"), Duration::from_secs(15));
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000002"
    );
    until(|| lab.count(0) == 2, Duration::from_secs(10));
    lab.stop(2);
    lab.stop(0);
    restore_store(&saved);
    assert_eq!(lab.count(0), 2);
    assert_eq!(lab.count(1), 1);
    assert_eq!(lab.count(2), 1);
}

/// The window is evaluated after the event it learns, never before it. A receipt
/// that shows a peer ahead can arrive as the first thing this process learns
/// after the window's end: it must count, and refuse the append, rather than let
/// the window latch on what was known before it.
#[test]
fn a_receipt_that_shows_a_peer_ahead_counts_before_the_window_is_weighed() {
    let mut lab = Lab::new();
    older_copy_with_one_holder(&mut lab);
    // r2 and r1 exchange directly. r2 -> r0 passes through a relay that first
    // drops every connection, then holds each one; r0 -> r2 and r0 <-> r1 are
    // dropped, so r1 never receives r2:2 and r0 never pushes it back.
    let r2_to_r0 = Proxy::new(lab.configs[0].network.bind);
    let r0_to_r2 = Proxy::new(lab.configs[2].network.bind);
    let r0_to_r1 = Proxy::new(lab.configs[1].network.bind);
    let r1_to_r0 = Proxy::new(lab.configs[0].network.bind);
    for proxy in [&r2_to_r0, &r0_to_r2, &r0_to_r1, &r1_to_r0] {
        proxy.blocked.store(true, Ordering::SeqCst);
    }
    peer_mut(&mut lab.configs[2], "r0").endpoint = r2_to_r0.address;
    peer_mut(&mut lab.configs[0], "r2").endpoint = r0_to_r2.address;
    peer_mut(&mut lab.configs[0], "r1").endpoint = r0_to_r1.address;
    peer_mut(&mut lab.configs[1], "r0").endpoint = r1_to_r0.address;
    lab.start(0);
    lab.start(1);
    lab.configs[2].catch_up_window_ms = Some(3_000);
    let spawned = Instant::now();
    lab.start(2);
    until(
        || {
            let catch_up = lab.status(2)["catch_up"].clone();
            catch_up["peers_matched"] == json!(["r1"])
                && catch_up["peers_missing"] == json!(["r0"])
                && catch_up["peers_ahead"] == json!([])
        },
        Duration::from_secs(5),
    );
    // Two seconds in, r0 starts answering, 1.5 s late: the attempt that carries
    // the window's end is the one whose receipt tells that r0 holds r2:2.
    while spawned.elapsed() < Duration::from_millis(2_000) {
        thread::sleep(Duration::from_millis(10));
    }
    r2_to_r0.hold_ms.store(1_500, Ordering::SeqCst);
    r2_to_r0.blocked.store(false, Ordering::SeqCst);
    let learnt = Instant::now();
    loop {
        let catch_up = lab.status(2)["catch_up"].clone();
        if catch_up["peers_ahead"] == json!(["r0"]) {
            break;
        }
        assert!(
            learnt.elapsed() < Duration::from_secs(6),
            "r2 never learnt that r0 holds facts it lacks; a held attempt may have outlasted \
             the two-second read deadline on a loaded machine: {catch_up}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    // Well past the window's end, r2 still refuses to append: it knows r0 holds
    // a fact it lacks.
    while spawned.elapsed() < Duration::from_millis(4_500) {
        thread::sleep(Duration::from_millis(20));
    }
    let reply = lab.control_expect(2, boot_append("boot-after-window"));
    assert_eq!(reply["error"], "append_observation_catching_up", "{reply}");
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up"], false);
    assert_eq!(catch_up["caught_up_by"], Value::Null);
    assert_eq!(catch_up["peers_ahead"], json!(["r0"]));

    // Once r0 can push r2:2 back, r2 imports it and its next fact takes the
    // sequence after it. Nothing forked.
    r0_to_r2.blocked.store(false, Ordering::SeqCst);
    let (reply, _) = append_when_caught_up(
        &lab,
        2,
        &boot_append("boot-after-window"),
        Duration::from_secs(15),
    );
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000003"
    );
    assert_eq!(lab.status(2)["catch_up"]["caught_up_by"], "every_peer");
    assert_eq!(
        fact_value(&lab, 2, "r2:00000000000000000002"),
        fact_value(&lab, 0, "r2:00000000000000000002")
    );
    lab.stop_all();
}

/// The window counts only what this process learnt after its end. A peer it
/// caught up with earlier can receive, from the peer the window would forgive,
/// the very facts this store lacks; the fresh round at the window's end sees
/// that peer ahead and the append waits.
#[test]
fn the_window_weighs_only_evidence_taken_after_its_end() {
    let mut lab = Lab::new();
    older_copy_with_one_holder(&mut lab);
    // r2 -> r1 direct; r1 -> r2 through a relay that is closed later. r2 <-> r0
    // is dropped both ways: r0 is the peer the window would forgive. r0 -> r1 is
    // opened later, so that r1 receives r2:2 from r0 alone.
    let r1_to_r2 = Proxy::new(lab.configs[2].network.bind);
    let r2_to_r0 = Proxy::new(lab.configs[0].network.bind);
    let r0_to_r2 = Proxy::new(lab.configs[2].network.bind);
    let r0_to_r1 = Proxy::new(lab.configs[1].network.bind);
    let r1_to_r0 = Proxy::new(lab.configs[0].network.bind);
    for proxy in [&r2_to_r0, &r0_to_r2, &r0_to_r1, &r1_to_r0] {
        proxy.blocked.store(true, Ordering::SeqCst);
    }
    peer_mut(&mut lab.configs[1], "r2").endpoint = r1_to_r2.address;
    peer_mut(&mut lab.configs[2], "r0").endpoint = r2_to_r0.address;
    peer_mut(&mut lab.configs[0], "r2").endpoint = r0_to_r2.address;
    peer_mut(&mut lab.configs[0], "r1").endpoint = r0_to_r1.address;
    peer_mut(&mut lab.configs[1], "r0").endpoint = r1_to_r0.address;
    lab.start(0);
    lab.start(1);
    lab.configs[2].catch_up_window_ms = Some(4_000);
    let spawned = Instant::now();
    lab.start(2);
    until(
        || {
            let catch_up = lab.status(2)["catch_up"].clone();
            catch_up["peers_matched"] == json!(["r1"]) && catch_up["peers_missing"] == json!(["r0"])
        },
        Duration::from_secs(5),
    );
    // r1 can no longer reach r2 and receives r2:2 from r0, well before r2's
    // window ends: what r2 knows of r1 is now out of date.
    r1_to_r2.blocked.store(true, Ordering::SeqCst);
    r0_to_r1.blocked.store(false, Ordering::SeqCst);
    until(|| lab.count(1) == 2, Duration::from_secs(10));
    let received_after = spawned.elapsed();
    assert!(
        received_after < Duration::from_secs(4),
        "r1 received r2:2 after r2's window had ended ({received_after:?}): inconclusive"
    );
    // Past the window's end, the fresh round finds r1 ahead and r2 keeps refusing.
    assert_catching_up_for(&lab, 2, "boot-after-window", Duration::from_secs(4));
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up"], false, "{catch_up}");
    assert_eq!(catch_up["peers_ahead"], json!(["r1"]));
    assert_eq!(catch_up["peers_missing"], json!(["r0", "r1"]));
    assert_eq!(catch_up["latest_own_fact_appended_locally"], true);

    // r1 can push again: r2 imports r2:2, counts r1 caught up on a receipt taken
    // after the window's end, and takes the next sequence with r0 still out of
    // reach. The fact r1 holds is the one r2 holds.
    r1_to_r2.blocked.store(false, Ordering::SeqCst);
    let (reply, _) = append_when_caught_up(
        &lab,
        2,
        &boot_append("boot-after-window"),
        Duration::from_secs(20),
    );
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000003"
    );
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "window");
    assert_eq!(catch_up["peers_missing"], json!(["r0"]));
    assert_eq!(
        fact_value(&lab, 2, "r2:00000000000000000002"),
        fact_value(&lab, 1, "r2:00000000000000000002")
    );
    lab.stop_all();
}

/// Spawns a resident without waiting for it to answer.
fn spawn_only(lab: &mut Lab, i: usize) {
    lab.release_address(i);
    let path = lab.config_path(i);
    fs::write(&path, serde_json::to_vec(&lab.configs[i]).unwrap()).unwrap();
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(lab.stderr_log(i))
        .unwrap();
    lab.children[i] = Some(
        Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
            .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
}

/// One control request on a socket path, without the laboratory's harness: what
/// the entrypoint's helper does.
fn control_at(socket: &std::path::Path, request: &Value) -> Option<Value> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream
        .write_all(&serde_json::to_vec(request).unwrap())
        .ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug)]
struct EntrypointStart {
    replica: usize,
    bound_ms: Option<u128>,
    observed_ms: Option<u128>,
    catching_up: usize,
    attempts: usize,
    last: String,
    event_id: Option<String>,
    /// The readiness blockers this start read while it waited, in order.
    blockers: Vec<Value>,
    /// The start ended because the catch-up cannot end: a forked history.
    blocked_by_collision: bool,
}

/// The start of packaging/podmesh-manager/universe/entrypoint.sh, played here:
/// the control socket must be bound within the 25-second budget, then one
/// operation ID is appended until it is observed. `catching_up` is not a failure
/// and has no deadline, `uncertain` and `busy` are retried within a budget that
/// restarts after each catching-up answer, and any other answer is terminal. The
/// resident's catch-up state is read every five seconds while it waits, and a
/// catch-up blocked by an event identity collision ends the start: no import can
/// carry what that replica lacks, so waiting would never end.
/// `guard` bounds this test alone; the entrypoint has no such bound.
fn entrypoint_start(
    replica: usize,
    socket: std::path::PathBuf,
    operation: String,
    guard: Duration,
) -> EntrypointStart {
    let spawned = Instant::now();
    let budget = Duration::from_secs(25);
    let mut start = EntrypointStart {
        replica,
        bound_ms: None,
        observed_ms: None,
        catching_up: 0,
        attempts: 0,
        last: String::new(),
        event_id: None,
        blockers: Vec::new(),
        blocked_by_collision: false,
    };
    let mut checked: Option<Instant> = None;
    while !socket.exists() {
        if spawned.elapsed() >= budget {
            start.last = "control socket not bound".into();
            return start;
        }
        thread::sleep(Duration::from_millis(200));
    }
    start.bound_ms = Some(spawned.elapsed().as_millis());
    let request = json!({
        "operation": "append_observation",
        "operation_id": operation,
        "scope": format!("s{replica}"),
        "subject": "boot",
        "value": format!("{operation} value"),
    });
    let mut retry_since = Instant::now();
    loop {
        start.attempts += 1;
        let reply = control_at(&socket, &request);
        if let Some(reply) = &reply {
            if reply["response"]["result"] == "observed" {
                start.observed_ms = Some(spawned.elapsed().as_millis());
                start.event_id = reply["response"]["fact"]["event_id"]
                    .as_str()
                    .map(str::to_owned);
                return start;
            }
        }
        start.last = reply.map_or_else(|| "no answer".into(), |reply| reply.to_string());
        if start.last.contains("append_observation_catching_up") {
            start.catching_up += 1;
            retry_since = Instant::now();
            if checked.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
                checked = Some(Instant::now());
                let blocker = control_at(&socket, &json!({"operation": "status"}))
                    .map_or(Value::Null, |status| {
                        status["catch_up"]["blocked_by"].clone()
                    });
                start.blockers.push(blocker.clone());
                if blocker["reason"] == "identity_collision" {
                    start.blocked_by_collision = true;
                    start.last = format!("catch-up blocked by an identity collision: {blocker}");
                    return start;
                }
            }
        } else if !(start.last.contains("append_observation_uncertain")
            || start.last.contains("append_observation_busy"))
            || retry_since.elapsed() >= budget
        {
            return start;
        }
        if spawned.elapsed() >= guard {
            start.last = format!("test guard of {guard:?} reached: {}", start.last);
            return start;
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn seed_every_replica_then_stop(lab: &mut Lab) {
    lab.start_all();
    for i in 0..3 {
        lab.append(
            i,
            &format!("seed-{i}"),
            &format!("s{i}"),
            "seed",
            "seed value",
        );
    }
    assert_converged(lab, 3);
    lab.stop_all();
}

/// A replica that is catching up is running, not failed: it keeps exchanging, so
/// that the replicas started after it catch up with it, and its start waits for
/// its own boot fact however long that takes. Three starts thirty seconds apart,
/// each played by the universe entrypoint's contract, all end observed.
#[test]
fn staggered_starts_keep_exchanging_while_they_catch_up_and_all_are_observed() {
    // The start played below must not drift from the shipped entrypoint: its
    // boot loop keeps retrying a catching-up answer outside the start's budget,
    // which still bounds the uncertain and busy ones.
    let script = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/podmesh-manager/universe/entrypoint.sh"),
    )
    .unwrap();
    assert!(
        script.contains("`catching_up` is retried with no deadline"),
        "the entrypoint no longer states the readiness contract this test plays"
    );
    assert!(
        !script.contains("append_observation_busy\\|append_observation_catching_up"),
        "the entrypoint bounds catching up by the start's budget again"
    );
    assert!(script.contains("append_observation_uncertain\\|append_observation_busy"));
    let mut lab = Lab::new();
    seed_every_replica_then_stop(&mut lab);
    // The universe's settings: an exchange every second, a backoff to thirty
    // seconds, one incoming worker, the default fifteen-second window.
    for config in &mut lab.configs {
        config.interval_ms = 1_000;
        config.max_backoff_ms = 30_000;
        config.incoming_workers = 1;
    }
    let begin = Instant::now();
    let mut starts = Vec::new();
    for (i, offset) in [(0_usize, 0_u64), (1, 30_000), (2, 60_000)] {
        while begin.elapsed() < Duration::from_millis(offset) {
            thread::sleep(Duration::from_millis(20));
        }
        let _ = fs::remove_file(&lab.configs[i].control_socket);
        spawn_only(&mut lab, i);
        let socket = lab.configs[i].control_socket.clone();
        starts.push(thread::spawn(move || {
            entrypoint_start(i, socket, format!("boot-r{i}"), Duration::from_secs(150))
        }));
    }
    let starts: Vec<_> = starts
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    for start in &starts {
        println!("MEASURED staggered start (30 s apart, 15 s window): {start:?}");
    }
    for (i, start) in starts.iter().enumerate() {
        assert_eq!(start.replica, i);
        assert!(start.observed_ms.is_some(), "{start:?}");
        assert_eq!(
            start.event_id.as_deref(),
            Some(format!("r{i}:00000000000000000002").as_str()),
            "{start:?}"
        );
    }
    // The first two waited past the 25-second budget for a peer to come up, and
    // were still running and exchanging when it did.
    assert!(starts[0].observed_ms.unwrap() > 25_000, "{:?}", starts[0]);
    assert!(starts[0].catching_up > 0, "{:?}", starts[0]);
    assert!(starts[1].catching_up > 0, "{:?}", starts[1]);
    for i in 0..3 {
        assert!(
            lab.children[i]
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none(),
            "resident r{i} is no longer running"
        );
        assert_eq!(lab.status(i)["catch_up"]["appends_observed"], 1);
    }
    until(
        || (0..3).all(|i| lab.count(i) == 6),
        Duration::from_secs(60),
    );
    let digests: Vec<_> = (0..3)
        .map(|i| inspection(&lab, i).logical_history_sha256)
        .collect();
    assert!(digests.iter().all(|digest| *digest == digests[0]));
    for i in 0..3 {
        println!(
            "MEASURED staggered start r{i} catch-up: {}",
            lab.status(i)["catch_up"]
        );
    }
    lab.stop_all();
}

/// A replica whose history forked from a peer's never catches up: every import
/// from that peer is refused as an event identity collision and every push to it
/// is refused too, so it is reached and never caught up with, and no window
/// forgives a peer that answers. The resident names that in its status, and the
/// start ends instead of running for ever without being ready -- while a replica
/// whose peer is merely down waits, says so, and becomes ready.
#[test]
fn a_forked_replica_names_what_blocks_it_and_its_start_ends() {
    // The entrypoint played below must not drift from the shipped one on this
    // either: a catch-up blocked by a collision is terminal there too.
    let script = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/podmesh-manager/universe/entrypoint.sh"),
    )
    .unwrap();
    assert!(script.contains("reason=identity_collision*)"), "{script}");
    assert!(script.contains("CATCH-UP BLOCKED BY AN EVENT IDENTITY COLLISION"));
    let mut lab = Lab::new();
    older_copy_with_one_holder(&mut lab);

    // r2 restarts on its older copy while r0, the only holder of r2:2, is down:
    // it waits for a peer, says so, and its window makes it ready -- with the
    // fork this whole gate exists to bound.
    lab.configs[2].catch_up_window_ms = Some(2_000);
    lab.start(1);
    let _ = fs::remove_file(&lab.configs[2].control_socket);
    spawn_only(&mut lab, 2);
    let waiting = entrypoint_start(
        2,
        lab.configs[2].control_socket.clone(),
        "boot-late-peer".into(),
        Duration::from_secs(60),
    );
    println!("MEASURED start with a peer down: {waiting:?}");
    assert_eq!(
        waiting.event_id.as_deref(),
        Some("r2:00000000000000000002"),
        "{waiting:?}"
    );
    assert!(!waiting.blocked_by_collision, "{waiting:?}");
    assert_eq!(
        waiting.blockers.first().unwrap()["reason"],
        "waiting_for_peers",
        "{waiting:?}"
    );
    assert!(
        waiting.blockers.first().unwrap()["peers"]
            .as_array()
            .unwrap()
            .contains(&json!("r0")),
        "{waiting:?}"
    );
    assert_eq!(lab.status(2)["catch_up"]["blocked_by"], Value::Null);

    // r1 takes that fact, r0 comes back holding the other version of it, and the
    // two histories are now irreconcilable.
    until(|| lab.count(1) == 2, Duration::from_secs(15));
    lab.start(0);
    until(
        || {
            lab.status(0)["peers"]["r2"]["identity_collisions"]["imports_refused"]
                .as_u64()
                .unwrap()
                >= 1
        },
        Duration::from_secs(20),
    );

    // Restarted against peers that are both up, r2 cannot catch up with r0 and
    // says so; its start ends rather than waiting for ever.
    lab.stop(2);
    let _ = fs::remove_file(&lab.configs[2].control_socket);
    spawn_only(&mut lab, 2);
    let blocked = entrypoint_start(
        2,
        lab.configs[2].control_socket.clone(),
        "boot-forked".into(),
        Duration::from_secs(60),
    );
    println!("MEASURED start of a forked replica: {blocked:?}");
    assert!(blocked.observed_ms.is_none(), "{blocked:?}");
    assert!(blocked.blocked_by_collision, "{blocked:?}");
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up"], false);
    assert_eq!(catch_up["blocked_by"]["reason"], "identity_collision");
    assert_eq!(catch_up["blocked_by"]["peers"], json!(["r0"]));
    assert_eq!(
        catch_up["blocked_by"]["event_id"],
        "r2:00000000000000000002"
    );
    assert_eq!(catch_up["appends_observed"], 0);
    // The entrypoint kills the resident it refuses to run; so does this test.
    let mut child = lab.children[2].take().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    lab.stop(0);
    lab.stop(1);
}
