//! External-process and persisted-store qualification; no deployed HA claim.
use podmesh_registry_lab::{digest, Observation, Store};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const M: &str = "00000000-0000-4000-8000-000000000001";
const R: &str = "00000000-0000-4000-8000-000000000002";
const E: &str = "00000000-0000-4000-8000-000000000003";
fn dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "registry-stress-{}",
        std::fs::read_to_string("/proc/sys/kernel/random/uuid")
            .unwrap()
            .trim()
    ));
    std::fs::create_dir(&p).unwrap();
    p
}
fn body(seq: u64, prev: Option<String>) -> Vec<u8> {
    serde_json::to_vec(&Observation {
        format: "podmesh-registry-observation-lab/1".into(),
        mesh_uuid: M.into(),
        origin_replica_uuid: R.into(),
        producer_epoch_uuid: E.into(),
        sequence: seq,
        previous_event_id: prev,
        dependencies: vec![],
        event_type: "observation.reported".into(),
        resource_key: "host:a:counter".into(),
        observed_at: "2026-09-12T00:00:00Z".into(),
        evidence_digest: "a".repeat(64),
        reported_state: "unknown".into(),
    })
    .unwrap()
}
fn create(path: &Path) -> Store {
    let s = Store::open(path, M).unwrap();
    s.enroll(E, R, "host:a:").unwrap();
    s
}
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
#[ignore = "subprocess fixture, invoked by crash test"]
fn crash_writer() {
    let path = std::env::var("REGISTRY_STRESS_DB").unwrap();
    let s = Store::open(Path::new(&path), M).unwrap();
    let mut prev = None;
    for n in 1..=1000 {
        let b = body(n, prev);
        let req = format!("10000000-0000-4000-8000-{n:012x}");
        let id = s.submit(&req, &b).unwrap();
        println!("ACK {n} {id}");
        std::io::stdout().flush().unwrap();
        prev = Some(id);
    }
    std::thread::sleep(std::time::Duration::from_secs(30));
}
#[test]
fn sigkill_preserves_acknowledged_commits_and_request_mapping() {
    for stop_after in [1, 10, 100] {
        let p = dir();
        let db = p.join("a.sqlite");
        drop(create(&db));
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash_writer", "--ignored", "--nocapture"])
                .env("REGISTRY_STRESS_DB", &db)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut reader = BufReader::new(child.0.stdout.take().unwrap());
        let (send, receive) = std::sync::mpsc::channel();
        let reader_task = std::thread::spawn(move || loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if send.send(line).is_err() {
                break;
            }
        });
        let mut ack = vec![];
        while ack.len() < stop_after {
            let line = receive
                .recv_timeout(std::time::Duration::from_secs(15))
                .expect("bounded writer acknowledgment");
            if line.starts_with("ACK ") {
                ack.push(line.split_whitespace().nth(2).unwrap().to_string());
            }
        }
        child.0.kill().unwrap();
        let status = child.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
        reader_task.join().unwrap();
        // A separate SQLite connection observes persisted facts independently of the writer.
        let external = rusqlite::Connection::open(&db).unwrap();
        let integrity: String = external
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        for id in &ack {
            let n: i64 = external
                .query_row("SELECT count(*) FROM events WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(n, 1);
        }
        let before: i64 = external
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        let requests_before: i64 = external
            .query_row("SELECT count(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(requests_before, before);
        let inconsistent:i64=external.query_row("SELECT count(*) FROM requests r LEFT JOIN events e ON e.id=r.event WHERE e.id IS NULL OR e.body != r.body",[],|r|r.get(0)).unwrap();
        assert_eq!(inconsistent, 0);
        let mut previous = None;
        for (i, id) in ack.iter().enumerate() {
            let request = format!("10000000-0000-4000-8000-{:012x}", i + 1);
            let saved: (Vec<u8>, String) = external
                .query_row(
                    "SELECT body,event FROM requests WHERE epoch=?1 AND request=?2",
                    rusqlite::params![E, request],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(saved, (body(i as u64 + 1, previous), id.clone()));
            previous = Some(id.clone());
        }
        let s = Store::open(&db, M).unwrap();
        let mut prev = None;
        for (i, id) in ack.iter().enumerate() {
            let b = body(i as u64 + 1, prev);
            assert_eq!(
                &s.submit(&format!("10000000-0000-4000-8000-{:012x}", i + 1), &b)
                    .unwrap(),
                id
            );
            prev = Some(id.clone());
        }
        assert_eq!(s.export().unwrap().len(), before as usize);
        let requests_after: i64 = external
            .query_row("SELECT count(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(requests_after, requests_before);
        assert!(s.states().unwrap().values().all(|v| v == "admitted"));
        drop(s);
        drop(external);
        std::fs::remove_dir_all(p).unwrap();
    }
}
#[test]
fn three_persistent_stores_rejoin_with_gaps_duplicates_and_load() {
    let p = dir();
    let paths = [p.join("a.sqlite"), p.join("b.sqlite"), p.join("c.sqlite")];
    let stores: Vec<_> = paths.iter().map(|p| create(p)).collect();
    let mut events = vec![];
    let mut prev = None;
    for seq in 1..=256 {
        let b = body(seq, prev);
        let id = digest(&b);
        prev = Some(id.clone());
        events.push((id, b));
    }
    for (id, b) in &events {
        stores[0].ingest(id, b).unwrap();
    }
    // B and C see disjoint batches in reverse order while offline.
    for (i, (id, b)) in events.iter().enumerate().rev() {
        stores[1 + i % 2].ingest(id, b).unwrap();
    }
    assert!(stores[1].states().unwrap().values().any(|s| s == "pending"));
    for store in &stores {
        for (id, b) in events.iter().rev() {
            store.ingest(id, b).unwrap();
            store.ingest(id, b).unwrap();
        }
    }
    let expected = stores[0].states().unwrap();
    assert_eq!(expected.len(), 256);
    assert!(expected.values().all(|s| s == "admitted"));
    drop(stores);
    for path in &paths {
        let s = Store::open(path, M).unwrap();
        assert_eq!(s.states().unwrap(), expected);
        assert_eq!(s.export().unwrap().len(), 256);
    }
    std::fs::remove_dir_all(p).unwrap();
}

#[test]
fn concurrent_duplicate_requests_converge_with_explicit_busy_retry() {
    let p = dir();
    let db = p.join("writers.sqlite");
    drop(create(&db));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let mut workers = vec![];
    let connections: Vec<_> = (0..4).map(|_| Store::open(&db, M).unwrap()).collect();
    for s in connections {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            // Open before the synchronized write race; setup is not under test here.
            barrier.wait();
            let bytes = body(1, None);
            for _ in 0..100 {
                match s.submit("10000000-0000-4000-8000-000000000001", &bytes) {
                    Ok(id) => return id,
                    Err(error) => {
                        let code = error
                            .downcast_ref::<rusqlite::Error>()
                            .and_then(|e| e.sqlite_error_code());
                        assert!(
                            matches!(
                                code,
                                Some(rusqlite::ErrorCode::DatabaseBusy)
                                    | Some(rusqlite::ErrorCode::DatabaseLocked)
                            ),
                            "unexpected error: {error}"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }
            panic!("explicit retry budget exhausted");
        }));
    }
    for worker in workers {
        assert_eq!(worker.join().unwrap(), digest(&body(1, None)));
    }
    let external = rusqlite::Connection::open(&db).unwrap();
    for table in ["events", "requests"] {
        let n: i64 = external
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
    drop(external);
    std::fs::remove_dir_all(p).unwrap();
}
