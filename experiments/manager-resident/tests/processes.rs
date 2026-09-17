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
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

struct Proxy {
    address: SocketAddr,
    blocked: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Proxy {
    fn new(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let blocked = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let block = Arc::clone(&blocked);
        let worker = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                if let Ok((mut source, _)) = listener.accept() {
                    if block.load(Ordering::SeqCst) {
                        drop(source);
                    } else {
                        workers.push(thread::spawn(move || {
                            if let Ok(mut destination) =
                                TcpStream::connect_timeout(&target, Duration::from_millis(100))
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

struct Lab {
    _dir: tempfile::TempDir,
    configs: Vec<Configuration>,
    children: Vec<Option<Child>>,
}
impl Lab {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        // Reserve the exact endpoint tuples that each child will bind, rather
        // than selecting a port on another loopback address and reusing it.
        let listeners: Vec<_> = (0..3)
            .map(|i| {
                TcpListener::bind((
                    std::net::Ipv4Addr::new(127, 0, 0, u8::try_from(i + 10).unwrap()),
                    0,
                ))
                .unwrap()
            })
            .collect();
        let addresses: Vec<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap())
            .collect();
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
        drop(listeners);
        Self {
            _dir: dir,
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
    fn start(&mut self, i: usize) {
        self.start_with_stderr(i, Stdio::inherit());
    }
    fn start_with_stderr(&mut self, i: usize, stderr: Stdio) {
        let path = self._dir.path().join(format!("r{i}.json"));
        fs::write(&path, serde_json::to_vec(&self.configs[i]).unwrap()).unwrap();
        self.children[i] = Some(
            Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
                .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
                .arg(path)
                .stdout(Stdio::null())
                .stderr(stderr)
                .spawn()
                .unwrap(),
        );
        until(
            || {
                if let Some(exit) = self.children[i].as_mut().unwrap().try_wait().unwrap() {
                    panic!("resident r{i} exited while starting: {exit}");
                }
                self.control(i, "status").is_some()
            },
            Duration::from_secs(15),
        );
    }
    fn status(&self, i: usize) -> Value {
        let start = Instant::now();
        loop {
            if let Some(reply) = self.control(i, "status") {
                if reply["kind"] == "resident_observation" {
                    return reply;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "status did not become available"
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
    fn control_bytes(&self, i: usize, request: &[u8]) -> Option<Vec<u8>> {
        let mut s = UnixStream::connect(&self.configs[i].control_socket).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        s.write_all(request).ok()?;
        s.shutdown(Shutdown::Write).ok()?;
        let mut bytes = Vec::new();
        s.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }
    fn append(
        &self,
        i: usize,
        operation_id: &str,
        scope: &str,
        subject: &str,
        value: &str,
    ) -> Value {
        let request = json!({
            "operation": "append_observation",
            "operation_id": operation_id,
            "scope": scope,
            "subject": subject,
            "value": value,
        });
        let start = Instant::now();
        loop {
            let reply = self.control_value(i, request.clone()).unwrap();
            if reply["response"]["result"] == "observed" {
                return reply;
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
            self.control(i, "shutdown").unwrap()["shutdown_requested"],
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
        lab.control_value(
            1,
            json!({"operation":"append_observation","operation_id":"refused-peer","scope":"s1","subject":"subject","value":"value"}),
        )
        .unwrap()["error"],
        "append_observation_catching_up"
    );
    let mut stream = TcpStream::connect(lab.configs[1].network.bind).unwrap();
    stream.write_all(&524_289_u32.to_be_bytes()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    assert_eq!(lab.count(1), 0);
    for i in 0..3 {
        lab.stop(i);
    }
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
        let reply = lab
            .control_value(0, json!({"operation":operation,"unexpected":true}))
            .unwrap();
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
    let first = lab.append(0, "append.1:local", "s0", "subject-1", "value");
    assert_eq!(first["response"]["result"], "observed");
    assert_eq!(first["response"]["fact"]["exclusive_resource"], Value::Null);
    assert_eq!(first["response"]["fact"]["active_claim"], false);
    assert_eq!(first["receipt"]["kind"], "observe");
    assert_eq!(first["replayed"], false);
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
        let reply = lab.control_value(0, request).unwrap();
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
        .control_value(
            0,
            json!({"operation":"append_observation","operation_id":"uid-refusal","scope":"s0","subject":"subject","value":"value"}),
        )
        .unwrap();
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
        .control_value(
            0,
            json!({"operation":"append_observation","operation_id":"disconnect-replay","scope":"s0","subject":"subject","value":"changed"}),
        )
        .unwrap();
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
    let started = Instant::now();
    let reply = lab.control_value(0, request.clone()).unwrap();
    assert_eq!(reply["error"], "append_observation_uncertain");
    assert!(started.elapsed() < Duration::from_millis(300));
    let started = Instant::now();
    let retry = lab.control_value(0, request.clone()).unwrap();
    assert_eq!(retry["error"], "append_observation_busy");
    assert!(started.elapsed() < Duration::from_millis(300));
    let second = json!({
        "operation":"append_observation",
        "operation_id":"busy-second-operation",
        "scope":"s0",
        "subject":"subject",
        "value":"second",
    });
    assert_eq!(
        lab.control_value(0, second.clone()).unwrap()["error"],
        "append_observation_busy"
    );
    let started = Instant::now();
    assert_eq!(
        lab.control(0, "shutdown").unwrap()["shutdown_requested"],
        true
    );
    assert!(started.elapsed() < Duration::from_millis(300));
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
    let replay = lab.control_value(0, request).unwrap();
    assert_eq!(replay["replayed"], true);
    let observed = lab.control_value(0, second).unwrap();
    assert_eq!(observed["response"]["result"], "observed");
    assert_eq!(lab.count(0), 2);
    lab.stop_all();
}

#[test]
fn status_remains_available_during_stalled_inbound_and_down_peer_attempts() {
    let mut lab = Lab::new();
    let passive = TcpListener::bind(lab.configs[0].network.peers[0].endpoint).unwrap();
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
    // elapsed, however slow a loaded machine makes each exchange.
    until(
        || peer_successes(&lab, 0, "r1") >= first + 3,
        Duration::from_secs(15),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1_900),
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
    let log = lab._dir.path().join("r0.stderr");
    lab.start_with_stderr(0, Stdio::from(fs::File::create(&log).unwrap()));
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
        || {
            lab.control_value(0, request.clone()).unwrap()["error"]
                == "append_observation_uncertain"
        },
        Duration::from_secs(10),
    );
    // Failed closed for the rest of the process, while status stays available.
    for _ in 0..5 {
        assert_eq!(
            lab.control_value(0, request.clone()).unwrap()["error"],
            "append_observation_uncertain"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(lab.status(0)["kind"], "resident_observation");
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
        (Some(20_000), true),
        (Some(999), false),
        (Some(20_001), false),
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
        let reply = lab.control_value(i, request.clone()).unwrap();
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
            lab.control_value(2, boot_append("boot-4")).unwrap()["error"],
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

#[test]
fn with_a_peer_down_only_a_store_that_held_its_own_facts_appends_after_the_window() {
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
    let refused_until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < refused_until {
        assert_eq!(
            lab.control_value(2, boot_append("boot-3")).unwrap()["error"],
            "append_observation_catching_up"
        );
        thread::sleep(Duration::from_millis(200));
    }
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up"], false);
    assert_eq!(catch_up["peers_imported"], json!(["r0"]));
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));
    assert_eq!(catch_up["own_facts_at_start"], 0);
    assert_eq!(catch_up["window_ms"], 2_000);
    assert_eq!(lab.count(2), 2);

    // The same store now holds its own two facts: after a restart it appends
    // once the window has elapsed, with the next sequence.
    lab.stop(2);
    let spawned = Instant::now();
    lab.start(2);
    let (reply, catching_up) =
        append_when_caught_up(&lab, 2, &boot_append("boot-3"), Duration::from_secs(10));
    assert!(spawned.elapsed() >= Duration::from_secs(2));
    assert!(catching_up > 0);
    assert_eq!(
        reply["response"]["fact"]["event_id"],
        "r2:00000000000000000003"
    );
    let catch_up = lab.status(2)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "window");
    assert_eq!(catch_up["caught_up_after_ms"], 2_000);
    assert_eq!(catch_up["own_facts_at_start"], 2);
    assert_eq!(catch_up["peers_missing"], json!(["r1"]));

    // r1 comes back and nothing collides.
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
    assert_eq!(catch_up["window_ms"], 15_000);
    assert_eq!(catch_up["peers_missing"], json!([]));
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
    let log = lab._dir.path().join("r0.stderr");
    let spawned = Instant::now();
    lab.start_with_stderr(0, Stdio::from(fs::File::create(&log).unwrap()));
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
        lab.control_value(
            0,
            json!({"operation":"append_observation","operation_id":"after-edit","scope":"s0","subject":"subject","value":"value"}),
        )
        .unwrap()["error"],
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
        .control_value(
            0,
            json!({"operation":"append_observation","operation_id":"alone","scope":"s0","subject":"subject","value":"value"}),
        )
        .unwrap();
    assert_eq!(reply["response"]["result"], "observed", "{reply}");
    let catch_up = lab.status(0)["catch_up"].clone();
    assert_eq!(catch_up["caught_up_by"], "no_peers");
    assert_eq!(catch_up["caught_up_after_ms"], 0);
    assert_eq!(catch_up["peers_missing"], json!([]));
    lab.stop(0);
}
