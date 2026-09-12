//! Real child-process requests and independently inspected disposable SQLite stores.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use podmesh_manager_ha_lab::{
    durable::{Configuration, Request, Response, Snapshot, Store},
    ConflictKind, Fact, ReplicaConfig, ScopeGrant,
};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Lab {
    directory: TempDir,
    config_path: PathBuf,
}

impl Lab {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let configuration = Configuration {
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
        };
        let config_path = directory.path().join("configuration.json");
        fs::write(&config_path, serde_json::to_vec(&configuration).unwrap()).unwrap();
        Self {
            directory,
            config_path,
        }
    }

    fn database(&self, replica: &str) -> PathBuf {
        self.directory.path().join(format!("{replica}.sqlite"))
    }

    fn output(&self, replica: &str, request: &Request) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-ha-lab"))
            .args([
                self.database(replica).as_os_str(),
                self.config_path.as_os_str(),
                replica.as_ref(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).unwrap())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn run(&self, replica: &str, request: &Request) -> Response {
        let output = self.output(replica, request);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn refused(&self, replica: &str, request: &Request) {
        let output = self.output(replica, request);
        assert!(!output.status.success());
        assert!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["error"]
                .is_string()
        );
    }

    fn snapshot(&self, replica: &str) -> Snapshot {
        let Response::Snapshot { snapshot } = self.run(replica, &Request::Export {}) else {
            panic!("snapshot expected")
        };
        snapshot
    }

    fn import(&self, replica: &str, snapshot: Snapshot, operation_id: &str) -> Response {
        self.run(
            replica,
            &Request::Import {
                operation_id: operation_id.into(),
                snapshot,
            },
        )
    }

    fn converge(&self, label: &str) {
        let snapshots: Vec<_> = ["r1", "r2", "r3"].map(|id| self.snapshot(id)).into();
        for id in ["r1", "r2", "r3"] {
            for snapshot in &snapshots {
                self.import(
                    id,
                    snapshot.clone(),
                    &format!("{label}-{}", snapshot.replica_id),
                );
            }
        }
    }

    fn check(replica: &str, snapshots: &[Snapshot]) -> Request {
        Request::CheckService {
            snapshots: snapshots
                .iter()
                .filter(|s| s.replica_id != replica)
                .cloned()
                .collect(),
            exclusive_service: "manager-ip".into(),
        }
    }
}

fn observation(n: u32, id: &str, value: &str, active: bool) -> Request {
    Request::Observe {
        operation_id: id.into(),
        scope: format!("scope{n}"),
        subject: "universe".into(),
        exclusive_resource: active.then(|| "manager-ip".into()),
        active_claim: active,
        value: value.into(),
    }
}

#[test]
fn committed_state_restarts_with_identical_retry_and_monotonic_sequence() {
    let lab = Lab::new();
    let request = observation(1, "write-1", "first", false);
    let first = lab.run("r1", &request);
    assert_eq!(lab.run("r1", &request), first);
    lab.refused("r1", &observation(1, "write-1", "changed", false));
    let Response::Observed { fact: second } =
        lab.run("r1", &observation(1, "write-2", "second", false))
    else {
        panic!()
    };
    assert_eq!(second.producer_sequence, 2);
    assert_eq!(second.subject_revision, 2);
    let database = Connection::open(lab.database("r1")).unwrap();
    let count: u32 = database
        .query_row("SELECT count(*) FROM facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(lab.snapshot("r1").facts.len(), 2);
    assert!(database.execute("DELETE FROM facts", []).is_err());
    assert!(database
        .execute("UPDATE identity SET replica_id='r2'", [])
        .is_err());
}

#[test]
fn partition_stale_copy_and_reconnect_preserve_newer_facts_before_coordination() {
    let lab = Lab::new();
    let stale = lab.snapshot("r1");
    lab.run("r2", &observation(2, "b1", "old", false));
    lab.run("r2", &observation(2, "b2", "new", false));
    lab.run("r3", &observation(3, "c1", "independent", false));
    lab.refused("r2", &observation(1, "cross-scope", "forbidden", false));
    let snapshots = [stale, lab.snapshot("r2"), lab.snapshot("r3")];
    lab.refused("r1", &Lab::check("r1", &snapshots));
    lab.converge("reconnect");
    let snapshots = ["r1", "r2", "r3"].map(|id| lab.snapshot(id));
    assert_eq!(snapshots[0].facts, snapshots[1].facts);
    assert_eq!(snapshots[1].facts, snapshots[2].facts);
    let Response::Inspection { current, .. } = lab.run("r1", &Request::Inspect {}) else {
        panic!()
    };
    assert!(current
        .iter()
        .any(|fact| fact.value == "new" && fact.subject_revision == 2));
    assert!(!current.iter().any(|fact| fact.value == "old"));
    let Response::ServiceCheck {
        coordinator_replica_id,
        eligible_in_supplied_history,
    } = lab.run("r1", &Lab::check("r1", &snapshots))
    else {
        panic!()
    };
    assert_eq!(coordinator_replica_id, "r1");
    assert!(!eligible_in_supplied_history);
}

#[test]
fn exactly_one_eligible_replica_per_reconciled_history_and_no_stale_local_substitution() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "claim", "claimed", true));
    lab.converge("first");
    let snapshots = ["r1", "r2", "r3"].map(|id| lab.snapshot(id));
    let eligible = ["r1", "r2", "r3"]
        .into_iter()
        .filter(|id| {
            matches!(
                lab.run(id, &Lab::check(id, &snapshots)),
                Response::ServiceCheck {
                    eligible_in_supplied_history: true,
                    ..
                }
            )
        })
        .count();
    assert_eq!(eligible, 1);
    lab.run("r1", &observation(1, "withdraw", "withdrawn", false));
    lab.refused("r1", &Lab::check("r1", &snapshots));
    lab.converge("withdrawn");
    let current = ["r1", "r2", "r3"].map(|id| lab.snapshot(id));
    for id in ["r1", "r2", "r3"] {
        assert!(matches!(
            lab.run(id, &Lab::check(id, &current)),
            Response::ServiceCheck {
                eligible_in_supplied_history: false,
                ..
            }
        ));
    }
}

#[test]
fn exclusive_conflicts_survive_restart_and_block_every_copy() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "claim1", "claim", true));
    lab.run("r2", &observation(2, "claim2", "competing claim", true));
    lab.converge("conflict");
    let snapshots = ["r1", "r2", "r3"].map(|id| lab.snapshot(id));
    for id in ["r1", "r2", "r3"] {
        let Response::Inspection {
            history_len,
            conflicts,
            blocked_exclusive_resources,
            ..
        } = lab.run(id, &Request::Inspect {})
        else {
            panic!()
        };
        assert_eq!(history_len, 2);
        assert_eq!(conflicts[0].kind, ConflictKind::ExclusiveResource);
        assert_eq!(blocked_exclusive_resources, ["manager-ip"]);
        assert!(matches!(
            lab.run(id, &Lab::check(id, &snapshots)),
            Response::ServiceCheck {
                eligible_in_supplied_history: false,
                ..
            }
        ));
    }
}

#[test]
fn import_is_atomic_idempotent_and_checks_topology_and_event_identity() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "a1", "first", false));
    lab.run("r1", &observation(1, "a2", "second", false));
    let snapshot = lab.snapshot("r1");
    let mut corrupt = snapshot.clone();
    let mut collision = corrupt.facts[0].clone();
    collision.value = "different bytes".into();
    corrupt.facts.push(collision);
    lab.refused(
        "r2",
        &Request::Import {
            operation_id: "invalid".into(),
            snapshot: corrupt,
        },
    );
    assert!(lab.snapshot("r2").facts.is_empty());
    let imported = lab.import("r2", snapshot.clone(), "import");
    assert_eq!(lab.import("r2", snapshot.clone(), "import"), imported);
    assert!(matches!(
        lab.import("r2", snapshot.clone(), "repeat"),
        Response::Imported {
            inserted: 0,
            history_len: 2
        }
    ));
    let mut foreign = snapshot;
    foreign.configuration.logical_manager_id = "foreign".into();
    lab.refused(
        "r2",
        &Request::Import {
            operation_id: "foreign".into(),
            snapshot: foreign,
        },
    );
    assert_eq!(lab.snapshot("r2").facts.len(), 2);
}

#[test]
fn incomplete_and_forked_histories_remain_quarantined_after_restart() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "a1", "first", true));
    lab.run("r1", &observation(1, "a2", "second", true));
    let mut snapshot = lab.snapshot("r1");
    snapshot.facts.remove(0);
    lab.import("r2", snapshot, "incomplete");
    let Response::Inspection { conflicts, .. } = lab.run("r2", &Request::Inspect {}) else {
        panic!()
    };
    assert_eq!(conflicts[0].kind, ConflictKind::MissingPredecessor);
    let mut fork = lab.snapshot("r1");
    let mut competing = fork.facts[1].clone();
    competing.producer_sequence = 3;
    competing.event_id = "r1:00000000000000000003".into();
    fork.facts.push(competing);
    lab.import("r2", fork, "fork");
    let Response::Inspection { conflicts, .. } = lab.run("r2", &Request::Inspect {}) else {
        panic!()
    };
    assert_eq!(conflicts[0].kind, ConflictKind::SubjectFork);
}

#[test]
fn concurrent_processes_cannot_overwrite_each_others_history_or_allocate_duplicate_sequences() {
    let lab = Lab::new();
    lab.snapshot("r1");
    thread::scope(|scope| {
        let mut threads = Vec::new();
        for index in 0..8 {
            let lab = &lab;
            threads.push(scope.spawn(move || {
                lab.run(
                    "r1",
                    &observation(1, &format!("write-{index}"), "concurrent", false),
                )
            }));
        }
        for handle in threads {
            handle.join().unwrap();
        }
    });
    let facts = lab.snapshot("r1").facts;
    assert_eq!(facts.len(), 8);
    assert_eq!(
        facts
            .iter()
            .map(|f| f.producer_sequence)
            .collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
    assert_eq!(facts.last().unwrap().subject_revision, 8);
}

#[test]
fn copied_database_cannot_be_opened_under_a_different_identity() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "original", "fact", false));
    // All request processes exited: this is an offline stale backup fixture only.
    fs::copy(lab.database("r1"), lab.database("r2")).unwrap();
    lab.refused("r2", &Request::Inspect {});
    assert_eq!(lab.snapshot("r1").facts.len(), 1);
}

#[test]
fn stale_backup_catches_up_without_reusing_origin_sequence() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "first", "old", false));
    let backup = lab.directory.path().join("stale.sqlite");
    fs::copy(lab.database("r1"), &backup).unwrap();
    lab.run("r1", &observation(1, "second", "new", false));
    lab.import("r2", lab.snapshot("r1"), "retain");
    fs::copy(backup, lab.database("r1")).unwrap();
    assert_eq!(lab.snapshot("r1").facts.len(), 1);
    let snapshots = [lab.snapshot("r1"), lab.snapshot("r2"), lab.snapshot("r3")];
    lab.refused("r1", &Lab::check("r1", &snapshots));
    lab.import("r1", lab.snapshot("r2"), "recover");
    let Response::Observed { fact } = lab.run("r1", &observation(1, "third", "recovered", false))
    else {
        panic!()
    };
    assert_eq!(fact.producer_sequence, 3);
    assert_eq!(fact.subject_revision, 3);
}

#[test]
fn corrupted_fact_and_unrecognized_database_fail_closed() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "first", "valid", false));
    let database = Connection::open(lab.database("r1")).unwrap();
    let mut fact = lab.snapshot("r1").facts[0].clone();
    fact.event_id = "r1:00000000000000000002".into();
    fact.producer_sequence = 2;
    database
        .execute(
            "INSERT INTO facts VALUES (?1, ?2, 'incorrect hash')",
            params![fact.event_id, serde_json::to_string(&fact).unwrap()],
        )
        .unwrap();
    lab.refused("r1", &Request::Inspect {});
    lab.refused("r1", &observation(1, "second", "must not commit", false));
    let count: u32 = database
        .query_row("SELECT count(*) FROM receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let other = Connection::open(lab.database("r2")).unwrap();
    other
        .execute("CREATE TABLE unrelated (value TEXT)", [])
        .unwrap();
    lab.refused("r2", &Request::Inspect {});
    let tables: u32 = other
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
}

#[test]
fn invalid_or_oversized_protocol_input_is_rejected_before_database_creation() {
    let lab = Lab::new();
    for input in [
        br#"{"operation":"inspect","unexpected":true}"#.to_vec(),
        vec![b' '; 1_048_577],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-ha-lab"))
            .args([
                lab.database("r1").as_os_str(),
                lab.config_path.as_os_str(),
                "r1".as_ref(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        assert!(!child.wait_with_output().unwrap().status.success());
        assert!(!lab.database("r1").exists());
    }
}

#[test]
fn maximum_imported_revision_is_quarantined_without_panicking_after_restart() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "seed", "fixture", true));
    let mut snapshot = lab.snapshot("r1");
    let first_id = snapshot.facts[0].event_id.clone();
    let second_id = "r1:00000000000000000002".to_string();
    snapshot.facts[0].subject_revision = u64::MAX;
    snapshot.facts[0].predecessor = Some(second_id.clone());
    let mut second = snapshot.facts[0].clone();
    second.event_id = second_id;
    second.producer_sequence = 2;
    second.subject_revision = 2;
    second.predecessor = Some(first_id);
    snapshot.facts.push(second);
    lab.import("r2", snapshot.clone(), "overflow-fixture");
    let inspected = lab.run("r2", &Request::Inspect {});
    let Response::Inspection {
        history_len,
        ref conflicts,
        ref current,
        ref blocked_exclusive_resources,
    } = inspected
    else {
        panic!()
    };
    assert_eq!(history_len, 2);
    assert!(current.is_empty());
    assert_eq!(conflicts[0].kind, ConflictKind::MissingPredecessor);
    assert_eq!(blocked_exclusive_resources, &["manager-ip"]);
    assert_eq!(lab.run("r2", &Request::Inspect {}), inspected);
    // Replace only a closed disposable fixture to test the owner after restore.
    fs::remove_file(lab.database("r1")).unwrap();
    lab.import("r1", snapshot, "restore-owner");
    lab.refused("r1", &observation(1, "must-not-extend", "blocked", false));
    assert_eq!(lab.snapshot("r1").facts.len(), 2);
}

#[test]
fn corrupted_receipt_response_cannot_produce_a_false_replay() {
    let lab = Lab::new();
    let request = observation(1, "original", "retained", false);
    let Response::Observed { mut fact } = lab.run("r1", &request) else {
        panic!()
    };
    fact.value = "fabricated response".into();
    let database = Connection::open(lab.database("r1")).unwrap();
    database
        .execute_batch("DROP TRIGGER receipts_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE receipts SET response_json=?1 WHERE operation_id='original'",
            [serde_json::to_string(&Response::Observed { fact }).unwrap()],
        )
        .unwrap();
    lab.refused("r1", &request);
    lab.refused("r1", &Request::Inspect {});
    lab.refused("r1", &observation(1, "new", "must not write", false));
    let count: u32 = database
        .query_row("SELECT count(*) FROM facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn receipt_checksum_binds_operation_id_and_request_against_fabricated_rows() {
    for changed_field in ["operation_id", "request_json"] {
        let lab = Lab::new();
        let request = observation(1, "original", "retained", false);
        lab.run("r1", &request);
        let database = Connection::open(lab.database("r1")).unwrap();
        if changed_field == "operation_id" {
            // Inserting a row does not invoke the immutable-update triggers. A
            // copied checksum must not authenticate a fabricated operation ID.
            database.execute(
                "INSERT INTO receipts SELECT 'fabricated', request_json, response_json, sha256 FROM receipts WHERE operation_id='original'", [],
            ).unwrap();
        } else {
            database
                .execute_batch("DROP TRIGGER receipts_no_update")
                .unwrap();
            database
                .execute(
                    "UPDATE receipts SET request_json=?1 WHERE operation_id='original'",
                    [
                        serde_json::to_string(&observation(
                            1,
                            "original",
                            "different intent",
                            false,
                        ))
                        .unwrap(),
                    ],
                )
                .unwrap();
        }
        lab.refused("r1", &request);
        lab.refused("r1", &Request::Inspect {});
        lab.refused("r1", &observation(1, "new", "must not write", false));
        let count: u32 = database
            .query_row("SELECT count(*) FROM facts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[test]
fn receipt_framing_handles_embedded_delimiters_and_preserves_verified_retry() {
    let lab = Lab::new();
    let request = observation(1, "operation\0\n\"\\|é", "value\0\n\"\\|é", false);
    let original = lab.run("r1", &request);
    let database = Connection::open(lab.database("r1")).unwrap();
    let (id, request_json, response_json, checksum): (String, String, String, String) = database
        .query_row(
            "SELECT operation_id, request_json, response_json, sha256 FROM receipts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let frame = serde_json::to_vec(&[
        "podmesh-manager-ha-receipt/1",
        &id,
        &request_json,
        &response_json,
    ])
    .unwrap();
    assert_eq!(checksum, format!("{:x}", Sha256::digest(frame)));
    assert_eq!(lab.run("r1", &request), original);
    assert_eq!(lab.snapshot("r1").facts.len(), 1);
}

#[test]
fn schema_without_receipt_integrity_version_is_refused_without_rewriting_it() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "original", "retained", false));
    let database = Connection::open(lab.database("r1")).unwrap();
    database.execute_batch("PRAGMA user_version=1").unwrap();
    lab.refused("r1", &Request::Inspect {});
    lab.refused("r1", &observation(1, "new", "must not write", false));
    let version: u32 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let count: u32 = database
        .query_row("SELECT count(*) FROM facts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn process_kill_recovers_uncommitted_transaction_and_committed_unacknowledged_retry() {
    let lab = Lab::new();
    lab.snapshot("r1");
    for phase in ["before_commit", "after_commit"] {
        let marker = lab.directory.path().join(phase);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "crash_worker", "--nocapture"])
            .env("MANAGER_LAB_CRASH_DB", lab.database("r1"))
            .env("MANAGER_LAB_CRASH_CONFIG", &lab.config_path)
            .env("MANAGER_LAB_CRASH_MARKER", &marker)
            .env("MANAGER_LAB_CRASH_PHASE", phase)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "worker exited before checkpoint"
            );
            thread::sleep(Duration::from_millis(10));
        }
        if !marker.exists() {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("checkpoint timeout");
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        let expected = usize::from(phase == "after_commit");
        assert_eq!(lab.snapshot("r1").facts.len(), expected);
        let database = Connection::open(lab.database("r1")).unwrap();
        assert_eq!(
            database
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM receipts", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            expected
        );
    }
    let replay = lab.run("r1", &observation(1, "crash-write", "committed", false));
    assert!(matches!(
        replay,
        Response::Observed {
            fact: Fact {
                producer_sequence: 1,
                ..
            }
        }
    ));
    assert_eq!(lab.snapshot("r1").facts.len(), 1);
}

// Test-only helper: the before-commit checkpoint holds a real SQLite transaction;
// the after-commit checkpoint executes the delivered Store API and loses its reply.
#[test]
#[ignore = "invoked as a crash-test child process"]
fn crash_worker() {
    let database = std::env::var("MANAGER_LAB_CRASH_DB").unwrap();
    let marker = std::env::var("MANAGER_LAB_CRASH_MARKER").unwrap();
    let configuration: Configuration = serde_json::from_slice(
        &fs::read(std::env::var("MANAGER_LAB_CRASH_CONFIG").unwrap()).unwrap(),
    )
    .unwrap();
    if std::env::var("MANAGER_LAB_CRASH_PHASE").unwrap() == "after_commit" {
        let mut store = Store::open(Path::new(&database), configuration, "r1").unwrap();
        store
            .execute(&observation(1, "crash-write", "committed", false))
            .unwrap();
    } else {
        let connection = Connection::open(database).unwrap();
        connection.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut replica = configuration.topology().unwrap().instantiate("r1").unwrap();
        let fact = replica
            .observe("scope1", "universe", None, false, "not committed")
            .unwrap();
        let encoded = serde_json::to_string(&fact).unwrap();
        let hash = format!("{:x}", Sha256::digest(encoded.as_bytes()));
        connection
            .execute(
                "INSERT INTO facts VALUES (?1, ?2, ?3)",
                params![fact.event_id, encoded, hash],
            )
            .unwrap();
        fs::write(marker, b"transaction open").unwrap();
        loop {
            thread::park();
        }
    }
    fs::write(marker, b"committed before reply").unwrap();
    loop {
        thread::park();
    }
}
