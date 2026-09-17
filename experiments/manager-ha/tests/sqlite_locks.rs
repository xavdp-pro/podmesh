//! A process that has a store open through SQLite never opens the store's files
//! around SQLite.
//!
//! POSIX advisory locks belong to a process and an inode: closing any descriptor of
//! the database, WAL or SHM file releases every lock the process holds on that file,
//! including the locks of its SQLite connections ("How To Corrupt An SQLite Database
//! File", section 2.2). Another process can then take the write lock, check the WAL
//! into the database under a snapshot this process still reads, or believe itself the
//! last connection and delete the WAL on close while this process keeps committing
//! into it. A store open reads the files itself only for its read-only preflight, so
//! these tests make an open meet a file state the process has not preflighted, as a
//! checkpoint by another process does, while the process keeps the store open.
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use podmesh_manager_ha_lab::{
    durable::{
        AuditDirection, AuditErrorCategory, AuditOutcome, AuditPhase, Configuration,
        ExchangeAuditEvent, RefusalReason, Request, Response, Store,
    },
    ReplicaConfig, ScopeGrant,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn configuration() -> Configuration {
    Configuration {
        logical_manager_id: "manager".into(),
        replicas: (1..=3)
            .map(|n| ReplicaConfig {
                replica_id: format!("r{n}"),
                host_id: format!("h{n}"),
            })
            .collect(),
        grants: (1..=3)
            .map(|n| ScopeGrant {
                scope: format!("scope{n}"),
                owner_replica_id: format!("r{n}"),
            })
            .collect(),
    }
}

fn open(path: &Path) -> Store {
    Store::open(path, configuration(), "r1").unwrap()
}

fn observation(operation_id: &str) -> Request {
    Request::Observe {
        operation_id: operation_id.into(),
        scope: "scope1".into(),
        subject: "locks".into(),
        exclusive_resource: None,
        active_claim: false,
        value: operation_id.into(),
    }
}

/// Observes through a store opened for this observation alone, as the resident's
/// control append does.
fn observe_with_a_new_store(path: &Path, operation_id: &str) -> bool {
    Store::open(path, configuration(), "r1")
        .and_then(|mut store| store.execute(&observation(operation_id)))
        .is_ok()
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

/// Gives the database file a modification time the process has not seen, as a
/// checkpoint by another process does, without opening the file in this process.
fn change_file_state_elsewhere(path: &Path) {
    assert!(Command::new("touch")
        .args(["-m", "-d", "2020-01-01"])
        .arg(path)
        .status()
        .unwrap()
        .success());
}

/// Runs one of this binary's ignored helper tests in a child process.
fn child(test: &str, store: &Path, extra: &[(&str, String)]) -> std::process::Output {
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            test,
            "--nocapture",
            "--test-threads=1",
        ])
        .env("MANAGER_LOCKS_STORE", store);
    for (name, value) in extra {
        command.env(name, value);
    }
    command.output().unwrap()
}

/// The values a helper printed after `marker`; the test harness may print its own
/// text before the marker on the same line.
fn marked<'a>(stdout: &'a str, marker: &'a str) -> impl Iterator<Item = &'a str> {
    stdout
        .lines()
        .filter_map(move |line| line.split_once(marker).map(|(_, value)| value.trim()))
}

fn helper_store() -> PathBuf {
    PathBuf::from(env::var_os("MANAGER_LOCKS_STORE").unwrap())
}

/// Another process reads the store with an ordinary read-write SQLite connection and
/// closes it, as any tool opening the live store does. Returns the facts it counted.
fn read_from_another_process(store: &Path) -> i64 {
    let output = child("helper_count_facts", store, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let facts = marked(&stdout, "FACTS ")
        .next()
        .unwrap_or_else(|| panic!("no count: {stdout}"))
        .parse()
        .unwrap();
    facts
}

#[test]
#[ignore = "invoked as a child process by the tests of this file"]
fn helper_count_facts() {
    let connection = Connection::open(helper_store()).unwrap();
    let facts: i64 = connection
        .query_row("SELECT count(*) FROM facts", [], |row| row.get(0))
        .unwrap();
    connection.close().unwrap();
    println!("FACTS {facts}");
}

/// Review of lot V2-S, probe R2c at the Store API: an open that copied the store
/// released this process's locks, one read by another process then deleted the WAL
/// and SHM at its close, and the commits this process made afterwards went into the
/// unlinked WAL, invisible to every other process.
#[test]
fn commits_after_another_open_stay_visible_to_other_processes() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("r1.sqlite");
    let mut kept = open(&path);
    kept.execute(&observation("kept-0")).unwrap();

    change_file_state_elsewhere(&path);
    assert!(observe_with_a_new_store(&path, "while-kept"));
    assert_eq!(read_from_another_process(&path), 2);
    assert!(
        sidecar(&path, "-wal").exists(),
        "another process deleted the WAL"
    );
    assert!(
        sidecar(&path, "-shm").exists(),
        "another process deleted the SHM"
    );

    kept.execute(&observation("kept-1")).unwrap();
    assert_eq!(read_from_another_process(&path), 3);
    assert!(kept.integrity().unwrap().failure.is_none());
}

/// Probe R3b: the resident's shape, a store kept open, a worker's store opened before
/// another open meets a changed file, stores opened per append, and another process
/// reading the live store; then the process crashes. Every append that the process
/// acknowledged must survive the crash.
#[test]
fn appends_acknowledged_around_another_open_survive_a_crash() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("r1.sqlite");
    let output = child("helper_crash_after_appends", &path, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "the helper did not crash: {stdout}"
    );
    let acknowledged: BTreeSet<String> = marked(&stdout, "ACKNOWLEDGED ")
        .map(str::to_string)
        .collect();

    // A new process finds every acknowledged append.
    let Response::Snapshot { snapshot } = open(&path).execute(&Request::Export {}).unwrap() else {
        panic!("export returned another response");
    };
    let stored: BTreeSet<String> = snapshot
        .facts
        .iter()
        .map(|fact| fact.value.clone())
        .collect();
    let lost: Vec<_> = acknowledged.difference(&stored).collect();
    assert!(
        lost.is_empty(),
        "acknowledged appends lost: {lost:?}: {stdout}"
    );

    // And nothing closed the store or refused an append in the crashed process.
    assert!(
        stdout.contains("STORE OPEN"),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for n in 1..=3 {
        for expected in [format!("worker-{n}"), format!("new-{n}")] {
            assert!(
                acknowledged.contains(&expected),
                "{expected} was not acknowledged: {stdout}"
            );
        }
    }
}

#[test]
#[ignore = "invoked as a child process by appends_acknowledged_around_another_open_survive_a_crash"]
fn helper_crash_after_appends() {
    let path = helper_store();
    let acknowledge = |operation_id: &str, done: bool| {
        if done {
            println!("ACKNOWLEDGED {operation_id}");
        }
    };
    let long_lived = open(&path);
    acknowledge("before", observe_with_a_new_store(&path, "before"));
    let mut worker = open(&path);
    acknowledge("worker-0", worker.execute(&observation("worker-0")).is_ok());
    change_file_state_elsewhere(&path);
    acknowledge("capture", observe_with_a_new_store(&path, "capture"));
    read_from_another_process(&path);
    for n in 1..=3 {
        let operation_id = format!("worker-{n}");
        acknowledge(
            &operation_id,
            worker.execute(&observation(&operation_id)).is_ok(),
        );
        let operation_id = format!("new-{n}");
        acknowledge(
            &operation_id,
            observe_with_a_new_store(&path, &operation_id),
        );
    }
    if long_lived.integrity().unwrap().failure.is_none() {
        println!("STORE OPEN");
    }
    std::process::abort();
}

fn prepared(label: &str) -> ExchangeAuditEvent {
    let hash = format!("{:x}", Sha256::digest(label.as_bytes()));
    ExchangeAuditEvent {
        audit_event_id: format!("audit-{}", &hash[..40]),
        attempt_id: format!("attempt:{hash}"),
        wire_nonce: format!("nonce-{}", &hash[..16]),
        direction: AuditDirection::Outbound,
        phase: AuditPhase::OutboundRequestPrepared,
        authenticated_peer_id: Some("r2".into()),
        peer_claim: Some("r2".into()),
        operation_id: Some(format!("operation-{}", &hash[..24])),
        request_frame_bytes: 0,
        request_announced_body_bytes: Some(5_531),
        request_sha256: Some(format!("{:064x}", 1)),
        reply_frame_bytes: 0,
        reply_announced_body_bytes: None,
        reply_sha256: None,
        outcome: AuditOutcome::Incomplete,
        error_category: None,
        reason_code: None,
        local_receipt_operation_id: None,
        local_receipt_sha256: None,
        remote_receipt_operation_id: None,
        remote_receipt_sha256: None,
        replayed: false,
    }
}

fn completed(first: &ExchangeAuditEvent) -> ExchangeAuditEvent {
    let mut completion = first.clone();
    completion.audit_event_id = format!("{}-done", first.audit_event_id);
    completion.phase = AuditPhase::OutboundExchangeCompleted;
    completion.outcome = AuditOutcome::Unavailable;
    completion.authenticated_peer_id = None;
    completion.error_category = Some(AuditErrorCategory::Unavailable);
    completion.reason_code = Some(RefusalReason::TransportUnavailable);
    completion
}

/// Probe A1, bounded: stores opened per attempt beside a store kept open, a reader
/// with periodic complete verifications, and another process that opens the live
/// store every 10 ms to read it, force a TRUNCATE checkpoint and close it. Nothing
/// stored is corrupt, so the store must never close; a busy store is expected and
/// closes nothing.
/// `PODMESH_MANAGER_LOCKS_SECONDS` lengthens the run.
#[test]
fn external_checkpoints_and_readers_close_nothing() {
    let seconds: u64 = env::var("PODMESH_MANAGER_LOCKS_SECONDS")
        .ok()
        .map_or(6, |value| value.parse().unwrap());
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("r1.sqlite");
    let long_lived = open(&path);
    {
        let mut seeding = open(&path);
        for index in 0..300 {
            let first = prepared(&format!("seed-{index}"));
            seeding.record_exchange_audit(&first).unwrap();
            seeding.record_exchange_audit(&completed(&first)).unwrap();
        }
    }
    let external = {
        let path = path.clone();
        thread::spawn(move || {
            child(
                "helper_read_and_checkpoint",
                &path,
                &[("MANAGER_LOCKS_SECONDS", seconds.to_string())],
            )
        })
    };
    let stop = Arc::new(AtomicBool::new(false));
    let counter = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut handles = Vec::new();
    for writer in 0..2 {
        let (stop, counter, errors, path) = (
            Arc::clone(&stop),
            Arc::clone(&counter),
            Arc::clone(&errors),
            path.clone(),
        );
        handles.push(thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let first = prepared(&format!("pressure-{writer}-{n}"));
                match Store::open(&path, configuration(), "r1") {
                    Ok(mut store) => {
                        for event in [first.clone(), completed(&first)] {
                            if let Err(problem) = store.record_exchange_audit(&event) {
                                errors.lock().unwrap().push(format!("audit: {problem}"));
                            }
                        }
                    }
                    Err(problem) => errors.lock().unwrap().push(format!("open: {problem}")),
                }
                thread::sleep(Duration::from_millis(1));
            }
        }));
    }
    {
        let (stop, errors, path) = (Arc::clone(&stop), Arc::clone(&errors), path.clone());
        handles.push(thread::spawn(move || {
            let mut store = open(&path);
            while !stop.load(Ordering::SeqCst) {
                if let Err(problem) = store.execute(&Request::Export {}) {
                    errors.lock().unwrap().push(format!("export: {problem}"));
                }
                if let Err(problem) = store.verify_full_if_due(Duration::from_millis(500)) {
                    errors.lock().unwrap().push(format!("pass: {problem}"));
                }
            }
        }));
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds)
        && long_lived.integrity().unwrap().failure.is_none()
    {
        thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::SeqCst);
    for handle in handles {
        handle.join().unwrap();
    }
    let external = external.join().unwrap();
    let failure = long_lived.integrity().unwrap().failure;
    let errors = errors.lock().unwrap();
    // A busy or locked store is expected under this pressure and closes nothing;
    // any other error is a verification failure this test refuses.
    let busy = |problem: &String| {
        problem.ends_with("database is locked") || problem.ends_with("database is busy")
    };
    assert!(
        failure.is_none(),
        "the store closed: {failure:?}; errors: {:?}",
        errors.iter().take(5).collect::<Vec<_>>()
    );
    let unexpected: Vec<_> = errors.iter().filter(|problem| !busy(problem)).collect();
    assert!(unexpected.is_empty(), "{unexpected:?}");
    assert!(
        external.status.success(),
        "{}{}",
        String::from_utf8_lossy(&external.stdout),
        String::from_utf8_lossy(&external.stderr)
    );
}

#[test]
#[ignore = "invoked as a child process by external_checkpoints_and_readers_close_nothing"]
fn helper_read_and_checkpoint() {
    let seconds: u64 = env::var("MANAGER_LOCKS_SECONDS").unwrap().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut checkpoints, mut busy) = (0, 0);
    while Instant::now() < deadline {
        let connection = Connection::open(helper_store()).unwrap();
        connection.busy_timeout(Duration::from_millis(100)).unwrap();
        let round = connection
            .query_row("SELECT count(*) FROM exchange_audit_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .and_then(|_| {
                connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    row.get::<_, i64>(0)
                })
            });
        match round {
            Ok(_) => checkpoints += 1,
            Err(_) => busy += 1,
        }
        drop(connection);
        thread::sleep(Duration::from_millis(10));
    }
    println!("external: {checkpoints} TRUNCATE checkpoints, {busy} busy");
}
