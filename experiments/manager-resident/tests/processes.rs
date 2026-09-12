use podmesh_manager_ha_lab::{
    durable::{Configuration as Manager, Request, Response, Store},
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
        let listeners: Vec<_> = (0..3)
            .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let addresses: Vec<_> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
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
                interval_ms: 100,
                max_backoff_ms: 400,
                incoming_workers: 2,
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
        let path = self._dir.path().join(format!("r{i}.json"));
        fs::write(&path, serde_json::to_vec(&self.configs[i]).unwrap()).unwrap();
        self.children[i] = Some(
            Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
                .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        until(
            || self.control(i, "status").is_some(),
            Duration::from_secs(5),
        );
    }
    fn control(&self, i: usize, operation: &str) -> Option<Value> {
        self.control_value(i, json!({"operation":operation}))
    }
    fn control_value(&self, i: usize, request: Value) -> Option<Value> {
        let mut s = UnixStream::connect(&self.configs[i].control_socket).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        s.write_all(&serde_json::to_vec(&request).unwrap()).ok()?;
        s.shutdown(Shutdown::Write).ok()?;
        let mut bytes = Vec::new();
        s.read_to_end(&mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    fn count(&self, i: usize) -> usize {
        match self.store(i).execute(&Request::Inspect {}).unwrap() {
            Response::Inspection { history_len, .. } => history_len,
            _ => panic!("unexpected inspect"),
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
}
impl Drop for Lab {
    fn drop(&mut self) {
        for child in self.children.iter_mut().flatten() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
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
    for i in 0..3 {
        lab.observe(i, "initial", None);
        lab.start(i);
    }
    until(
        || (0..3).all(|i| lab.count(i) == 3),
        Duration::from_secs(15),
    );
    for i in 0..3 {
        let status = lab.control(i, "status").unwrap();
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
    lab.observe(0, "during-partition", None);
    lab.observe(1, "during-partition", None);
    until(
        || lab.count(0) == 5 && lab.count(1) == 5,
        Duration::from_secs(10),
    );
    assert_eq!(lab.count(2), 3);
    until(
        || {
            let status = lab.control(0, "status").unwrap();
            let peer = &status["peers"]["r2"];
            peer["local_history_len_at_attempt"] == 5
                && peer["outcome"] != "authenticated_import_receipt"
        },
        Duration::from_secs(10),
    );
    assert!(lab.control(0, "status").unwrap()["peers"]["r2"]["history_count_delta"].is_null());
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
        assert!(
            lab.control(i, "status").unwrap()["inspection"]["blocked_exclusive_resources"]
                .as_array()
                .unwrap()
                .contains(&json!("ip:test"))
        );
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
            lab.control(0, "status").unwrap()["peers"]["r1"]["failures"]
                .as_u64()
                .unwrap()
                > 0
        },
        Duration::from_secs(10),
    );
    assert_eq!(lab.count(1), 0);
    assert_eq!(lab.count(2), 0);
    let status = lab.control(0, "status").unwrap();
    assert_eq!(status["peers"]["r1"]["authenticated_successes"], 0);
    assert_eq!(
        status["peers"]["r1"]["outcome"],
        "unauthenticated_remote_diagnostic"
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
        || {
            lab.control(0, "status").unwrap()["rejected_connections"]
                .as_u64()
                .unwrap()
                > 0
        },
        Duration::from_secs(4),
    );
    let status = lab.control(0, "status").unwrap();
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
    let mut value = serde_json::to_value(&lab.configs[0]).unwrap();
    value["arbitrary_command"] = json!("false");
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
    assert_eq!(
        lab.control(0, "status").unwrap()["kind"],
        "resident_observation"
    );
    lab.stop(0);
}

#[test]
fn store_failure_exit_unlinks_owned_control_socket() {
    let mut lab = Lab::new();
    lab.start(0);
    // Replace the configured path with a directory in this disposable fixture.
    // The next Store open deterministically fails, independently of permissions.
    let path = &lab.configs[0].network.database_path;
    fs::rename(path, path.with_extension("retained.sqlite")).unwrap();
    fs::create_dir(path).unwrap();
    // A status request may observe the store failure first; either this path or
    // the outgoing worker must stop admission and unlink its owned socket.
    let _ = lab.control(0, "status");
    let mut child = lab.children[0].take().unwrap();
    until(
        || child.try_wait().unwrap().is_some(),
        Duration::from_secs(15),
    );
    assert!(!child.wait().unwrap().success());
    assert!(!lab.configs[0].control_socket.exists());
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
    mac.update(b"podmesh-manager-network-lab/1\\0");
    mac.update(&bytes);
    let mac_hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let body=serde_json::to_vec(&json!({"protocol":protocol,"source_replica_id":"r0","destination_replica_id":"r1","operation_id":op,"nonce":nonce,"snapshot":snapshot,"mac_hex":mac_hex})).unwrap();
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
    assert_eq!(reply["result"], "diagnostic");
    assert!(
        !reply["detail"].as_str().unwrap().contains("authentication"),
        "the invalid batch must reach the authenticated durable validator"
    );
    assert_eq!(lab.count(1), 0);
    lab.stop(1);
}
