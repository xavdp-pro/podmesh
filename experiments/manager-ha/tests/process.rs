//! Real child-process requests and independently inspected disposable SQLite stores.
use std::{
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use podmesh_manager_ha_lab::{
    durable::{
        authenticated_import_receipt_id, inspect_read_only, AuditDirection, AuditErrorCategory,
        AuditOutcome, AuditPhase, Configuration, DurableError, ExchangeAuditEvent, ReceiptKind,
        RefusalReason, Request, Response, Snapshot, Store,
    },
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

fn audit(
    audit_event_id: &str,
    attempt_id: &str,
    phase: AuditPhase,
    outcome: AuditOutcome,
    operation_id: Option<&str>,
) -> ExchangeAuditEvent {
    let attempt_id = if attempt_id.starts_with("attempt:") {
        attempt_id.to_string()
    } else {
        format!("attempt:{:x}", Sha256::digest(attempt_id.as_bytes()))
    };
    let direction = if matches!(
        phase,
        AuditPhase::OutboundRequestPrepared | AuditPhase::OutboundExchangeCompleted
    ) {
        AuditDirection::Outbound
    } else {
        AuditDirection::Inbound
    };
    let authenticated = matches!(
        outcome,
        AuditOutcome::Accepted | AuditOutcome::AuthenticatedRefusal
    ) || matches!(
        phase,
        AuditPhase::InboundReplyPrepared | AuditPhase::InboundReplyWriteObserved
    );
    let (error_category, reason_code) = match outcome {
        AuditOutcome::AuthenticatedRefusal => (
            Some(AuditErrorCategory::Refused),
            Some(RefusalReason::PolicyViolation),
        ),
        AuditOutcome::Unavailable => (
            Some(AuditErrorCategory::Unavailable),
            Some(RefusalReason::TransportUnavailable),
        ),
        AuditOutcome::Malformed | AuditOutcome::UnauthenticatedDiagnostic => (
            Some(AuditErrorCategory::Malformed),
            Some(RefusalReason::InvalidRequest),
        ),
        AuditOutcome::Accepted | AuditOutcome::Incomplete => (None, None),
    };
    let request_transfer = matches!(
        phase,
        AuditPhase::OutboundExchangeCompleted | AuditPhase::InboundRequestObserved
    );
    let request_intent = request_transfer || phase == AuditPhase::OutboundRequestPrepared;
    let reply_transfer = matches!(
        phase,
        AuditPhase::OutboundExchangeCompleted
            | AuditPhase::InboundReplyWriteObserved
            | AuditPhase::InboundDiagnosticReplyWritten
    );
    let reply_intent = reply_transfer || phase == AuditPhase::InboundReplyPrepared;
    ExchangeAuditEvent {
        audit_event_id: audit_event_id.into(),
        wire_nonce: format!("wire-{}", &attempt_id[8..24]),
        attempt_id,
        direction,
        phase,
        authenticated_peer_id: authenticated.then(|| "r2".into()),
        peer_claim: Some("r2".into()),
        operation_id: operation_id.map(str::to_string),
        request_frame_bytes: if request_transfer { 132 } else { 0 },
        request_announced_body_bytes: request_intent.then_some(128),
        request_sha256: request_intent.then(|| format!("{:064x}", 1)),
        reply_frame_bytes: if reply_transfer { 68 } else { 0 },
        reply_announced_body_bytes: reply_intent.then_some(64),
        reply_sha256: reply_intent.then(|| format!("{:064x}", 2)),
        outcome,
        error_category,
        reason_code,
        local_receipt_operation_id: None,
        local_receipt_sha256: None,
        remote_receipt_operation_id: None,
        remote_receipt_sha256: None,
        replayed: false,
    }
}

fn preauth_audit(
    audit_event_id: &str,
    attempt_id: &str,
    phase: AuditPhase,
    outcome: AuditOutcome,
) -> ExchangeAuditEvent {
    let mut event = audit(audit_event_id, attempt_id, phase, outcome, None);
    event.wire_nonce = format!("preauth:{:x}", Sha256::digest(attempt_id.as_bytes()));
    event.authenticated_peer_id = None;
    event.peer_claim = None;
    event.operation_id = None;
    event
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
fn preflight_cache_does_not_accept_an_in_place_identity_replacement() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let r1_path = lab.database("r1");
    let r2_path = lab.database("r2");
    Store::open(&r1_path, configuration.clone(), "r1").unwrap();
    let marker = lab.directory.path().join("cache-r2-live");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash_worker", "--nocapture"])
        .env("MANAGER_LAB_CRASH_DB", &r2_path)
        .env("MANAGER_LAB_CRASH_CONFIG", &lab.config_path)
        .env("MANAGER_LAB_CRASH_MARKER", &marker)
        .env("MANAGER_LAB_CRASH_PHASE", "live_v3_wal")
        .env("MANAGER_LAB_CRASH_REPLICA", "r2")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        assert!(child.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let r2_wal = PathBuf::from(format!("{}-wal", r2_path.display()));
    let r2_shm = PathBuf::from(format!("{}-shm", r2_path.display()));
    assert!(fs::metadata(&r2_wal).unwrap().len() > 0);
    assert!(fs::metadata(&r2_shm).unwrap().len() > 0);
    let before_metadata = fs::metadata(&r1_path).unwrap();
    fs::copy(&r2_path, &r1_path).unwrap();
    fs::copy(&r2_wal, PathBuf::from(format!("{}-wal", r1_path.display()))).unwrap();
    fs::copy(&r2_shm, PathBuf::from(format!("{}-shm", r1_path.display()))).unwrap();
    let replaced_metadata = fs::metadata(&r1_path).unwrap();
    assert_eq!(before_metadata.ino(), replaced_metadata.ino());
    assert!(
        before_metadata.len() != replaced_metadata.len()
            || before_metadata.mtime_nsec() != replaced_metadata.mtime_nsec()
            || before_metadata.ctime_nsec() != replaced_metadata.ctime_nsec()
    );
    let before = directory_state(lab.directory.path());
    assert_eq!(
        Store::open(&r1_path, configuration, "r1").err().unwrap(),
        DurableError::Refused(RefusalReason::IdentityMismatch)
    );
    assert_eq!(directory_state(lab.directory.path()), before);
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
fn stored_fact_corruption_has_corrupt_class() {
    for case in ["fact-json", "fact-model"] {
        let lab = Lab::new();
        let configuration = store_configuration(&lab);
        let path = lab.database("r1");
        let store = Store::open(&path, configuration.clone(), "r1").unwrap();
        let database = Connection::open(&path).unwrap();
        let expected = match case {
            "fact-json" => {
                let encoded = "{invalid-json";
                database
                    .execute(
                        "INSERT INTO facts VALUES ('r1:00000000000000000001', ?1, ?2)",
                        params![encoded, format!("{:x}", Sha256::digest(encoded.as_bytes()))],
                    )
                    .unwrap();
                DurableError::Corrupt("stored fact JSON is invalid".into())
            }
            "fact-model" => {
                let fact = Fact {
                    event_id: "r2:00000000000000000001".into(),
                    logical_manager_id: "manager".into(),
                    origin_replica_id: "r2".into(),
                    origin_host_id: "h2".into(),
                    producer_sequence: 1,
                    scope: "scope1".into(),
                    subject: "invalid-scope-owner".into(),
                    subject_revision: 1,
                    predecessor: None,
                    exclusive_resource: None,
                    active_claim: false,
                    value: "invalid".into(),
                };
                let encoded = serde_json::to_string(&fact).unwrap();
                database
                    .execute(
                        "INSERT INTO facts VALUES (?1, ?2, ?3)",
                        params![
                            fact.event_id,
                            encoded,
                            format!("{:x}", Sha256::digest(encoded.as_bytes()))
                        ],
                    )
                    .unwrap();
                DurableError::Corrupt(
                    "stored model state is invalid: invalid or out-of-scope fact".into(),
                )
            }
            _ => unreachable!(),
        };
        drop(database);
        drop(store);
        assert_eq!(
            inspect_read_only(&path, &configuration, "r1").unwrap_err(),
            expected
        );
    }
}

#[test]
fn request_policy_failure_has_refused_class_without_mutation() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    assert_eq!(
        store
            .execute_with_receipt(&observation(2, "unowned-request", "refused", false))
            .unwrap_err(),
        DurableError::Refused(RefusalReason::PolicyViolation)
    );
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1")
            .unwrap()
            .history_count,
        0
    );
}

#[test]
fn invalid_local_configuration_is_not_a_signable_request_refusal() {
    let lab = Lab::new();
    let path = lab.database("r1");
    let configuration = store_configuration(&lab);
    Store::open(&path, configuration.clone(), "r1").unwrap();
    let mut invalid = configuration;
    invalid.replicas[1].replica_id = invalid.replicas[0].replica_id.clone();
    let open_error = Store::open(&lab.database("invalid"), invalid.clone(), "r1")
        .err()
        .unwrap();
    let inspect_error = inspect_read_only(&path, &invalid, "r1").unwrap_err();
    for problem in [open_error, inspect_error] {
        assert!(
            matches!(problem, DurableError::Storage(ref detail) if detail.starts_with("invalid local configuration: ")),
            "{problem:?}"
        );
    }
    assert!(!lab.database("invalid").exists());
}

#[test]
fn stored_audit_peer_corruption_has_corrupt_class() {
    for peer in ["r9", "r1"] {
        let lab = Lab::new();
        let configuration = store_configuration(&lab);
        let path = lab.database("r1");
        let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
        let prepared = audit(
            "corrupt-peer-audit",
            "corrupt-peer-attempt",
            AuditPhase::OutboundRequestPrepared,
            AuditOutcome::Incomplete,
            Some("peer-operation"),
        );
        store.record_exchange_audit(&prepared).unwrap();
        let mut changed = prepared;
        changed.authenticated_peer_id = Some(peer.into());
        changed.peer_claim = Some(peer.into());
        let record_json = serde_json::to_string(&changed).unwrap();
        let database = Connection::open(&path).unwrap();
        database
            .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
            .unwrap();
        database
            .execute(
                "UPDATE exchange_audit_events
                 SET authenticated_peer_id=?1, peer_claim=?1, record_json=?2, sha256=?3",
                params![peer, record_json, audit_checksum(&record_json)],
            )
            .unwrap();
        database
            .execute_batch(
                "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
            )
            .unwrap();
        drop(database);
        drop(store);
        let expected = if peer == "r9" {
            DurableError::Corrupt("stored audit authenticates an unknown peer".into())
        } else {
            DurableError::Corrupt("stored audit authenticates the local replica as its peer".into())
        };
        assert_eq!(
            inspect_read_only(&path, &configuration, "r1").unwrap_err(),
            expected
        );
    }
}

#[test]
fn unreadable_sqlite_content_has_storage_class_without_source_mutation() {
    let lab = Lab::new();
    let path = lab.database("r1");
    fs::write(&path, b"not a SQLite database").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        inspect_read_only(&path, &store_configuration(&lab), "r1"),
        Err(DurableError::Storage(_))
    ));
    assert_eq!(fs::read(path).unwrap(), before);
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
    database
        .execute_batch(
            "CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;",
        )
        .unwrap();
    assert_eq!(
        inspect_read_only(&lab.database("r1"), &store_configuration(&lab), "r1").unwrap_err(),
        DurableError::Corrupt("stored receipt hash mismatch".into())
    );
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
                "INSERT INTO receipts (operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256)
                 SELECT 'fabricated', kind, source_replica_id, wire_operation_id, request_json, response_json, sha256
                 FROM receipts WHERE operation_id='original'", [],
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
            database
                .execute_batch(
                    "CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;",
                )
                .unwrap();
        }
        let expected = if changed_field == "operation_id" {
            DurableError::Corrupt("stored receipt kind or source does not match its request".into())
        } else {
            DurableError::Corrupt("stored receipt hash mismatch".into())
        };
        assert_eq!(
            inspect_read_only(&lab.database("r1"), &store_configuration(&lab), "r1").unwrap_err(),
            expected
        );
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
    let request = observation(1, "operation-framing", "value\0\n\"\\|é", false);
    let original = lab.run("r1", &request);
    let database = Connection::open(lab.database("r1")).unwrap();
    let (id, request_json, response_json, checksum): (String, String, String, String) = database
        .query_row(
            "SELECT operation_id, request_json, response_json, sha256 FROM receipts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let frame = serde_json::to_vec(&serde_json::json!([
        "podmesh-manager-ha-receipt/2",
        "manager",
        &id,
        {
            "kind": "observe",
            "source_replica_id": null,
            "wire_operation_id": null
        },
        &request_json,
        &response_json,
    ]))
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
fn receipt_bearing_execution_returns_exact_evidence_on_retry() {
    let lab = Lab::new();
    let configuration: Configuration =
        serde_json::from_slice(&fs::read(&lab.config_path).unwrap()).unwrap();
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    let request = observation(1, "receipt-operation", "retained", false);
    let first = store.execute_with_receipt(&request).unwrap();
    assert!(!first.replayed);
    let receipt = first.receipt.clone().unwrap();
    assert_eq!(receipt.operation_id, "receipt-operation");
    assert_eq!(receipt.sha256.len(), 64);
    let replay = store.execute_with_receipt(&request).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.response, first.response);
    assert_eq!(replay.receipt, first.receipt);
    assert!(store
        .execute_with_receipt(&observation(1, "receipt-operation", "different", false))
        .is_err());
    let inspection =
        inspect_read_only(&lab.database("r1"), &store_configuration(&lab), "r1").unwrap();
    assert_eq!(inspection.history_count, 1);
    assert_eq!(inspection.receipt_count, 1);
}

#[test]
fn authenticated_import_commits_facts_receipt_and_audit_atomically_and_replays() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "source", "peer fact", false));
    let snapshot = lab.snapshot("r1");
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r2"), configuration.clone(), "r2").unwrap();
    let mut observed = audit(
        "audit-observed-first",
        "attempt-first",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("authenticated-import"),
    );
    observed.authenticated_peer_id = Some("r1".into());
    observed.peer_claim = Some("r1".into());
    store.record_exchange_audit(&observed).unwrap();
    let mut first_audit = audit(
        "audit-import-first",
        "attempt-first",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("authenticated-import"),
    );
    first_audit.authenticated_peer_id = Some("r1".into());
    first_audit.peer_claim = Some("r1".into());
    let identical_audit_retry = first_audit.clone();
    let first = store
        .execute_authenticated_import("authenticated-import", &snapshot, first_audit)
        .unwrap();
    assert!(!first.executed.replayed);
    assert_eq!(
        first.audit.event.local_receipt_operation_id.as_deref(),
        first
            .executed
            .receipt
            .as_ref()
            .map(|receipt| receipt.operation_id.as_str())
    );
    assert_eq!(
        first.audit.event.local_receipt_sha256,
        first
            .executed
            .receipt
            .as_ref()
            .map(|receipt| receipt.sha256.clone())
    );
    let same_audit = store
        .execute_authenticated_import("authenticated-import", &snapshot, identical_audit_retry)
        .unwrap();
    assert!(same_audit.executed.replayed);
    assert_eq!(same_audit.audit, first.audit);
    let mut replay_observed = audit(
        "audit-observed-retry",
        "attempt-retry",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("authenticated-import"),
    );
    replay_observed.authenticated_peer_id = Some("r1".into());
    replay_observed.peer_claim = Some("r1".into());
    store.record_exchange_audit(&replay_observed).unwrap();
    let mut replay_audit = audit(
        "audit-import-retry",
        "attempt-retry",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("authenticated-import"),
    );
    replay_audit.authenticated_peer_id = Some("r1".into());
    replay_audit.peer_claim = Some("r1".into());
    let replay = store
        .execute_authenticated_import("authenticated-import", &snapshot, replay_audit)
        .unwrap();
    assert!(replay.executed.replayed);
    assert!(replay.audit.event.replayed);
    assert_eq!(replay.executed.receipt, first.executed.receipt);
    let inspection = inspect_read_only(&lab.database("r2"), &configuration, "r2").unwrap();
    assert_eq!(inspection.history_count, 1);
    assert_eq!(inspection.receipt_count, 1);
    assert_eq!(inspection.audit_event_count, 4);
    assert!(inspection.unaudited_import_receipt_ids.is_empty());
    assert_eq!(inspection.incomplete_attempts.len(), 2);
    assert!(inspection
        .incomplete_attempts
        .iter()
        .all(|attempt| attempt.last_phase == AuditPhase::InboundImportCommitted));
}

#[test]
fn authenticated_import_rolls_back_facts_and_receipt_when_audit_refuses() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "source", "peer fact", false));
    let snapshot = lab.snapshot("r1");
    let configuration = store_configuration(&lab);
    let failed_database = lab.database("r3");
    let mut failed = Store::open(&failed_database, configuration.clone(), "r3").unwrap();
    let mut failed_observed = audit(
        "audit-collision",
        "attempt-must-fail",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("must-roll-back"),
    );
    failed_observed.authenticated_peer_id = Some("r1".into());
    failed_observed.peer_claim = Some("r1".into());
    failed.record_exchange_audit(&failed_observed).unwrap();
    let mut failed_audit = audit(
        "audit-collision",
        "attempt-must-fail",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("must-roll-back"),
    );
    failed_audit.authenticated_peer_id = Some("r1".into());
    failed_audit.peer_claim = Some("r1".into());
    assert_eq!(
        failed
            .execute_authenticated_import("must-roll-back", &snapshot, failed_audit)
            .unwrap_err(),
        DurableError::InvalidAudit("audit event ID reused with different bytes".into())
    );
    let inspection = inspect_read_only(&failed_database, &configuration, "r3").unwrap();
    assert_eq!(inspection.history_count, 0);
    assert_eq!(inspection.receipt_count, 0);
    assert_eq!(inspection.audit_event_count, 1);
}

#[test]
fn authenticated_import_requires_an_authenticated_accepted_observation() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "source-auth", "peer fact", false));
    let snapshot = lab.snapshot("r1");
    let configuration = store_configuration(&lab);
    let path = lab.database("r3");
    let mut store = Store::open(&path, configuration.clone(), "r3").unwrap();
    let mut observed = audit(
        "unauthenticated-observation",
        "unauthenticated-attempt",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::UnauthenticatedDiagnostic,
        Some("authenticated-after-unauthenticated"),
    );
    observed.peer_claim = Some("r1".into());
    store.record_exchange_audit(&observed).unwrap();
    let mut committed = audit(
        "invalid-authenticated-import",
        "unauthenticated-attempt",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("authenticated-after-unauthenticated"),
    );
    committed.authenticated_peer_id = Some("r1".into());
    committed.peer_claim = Some("r1".into());
    assert_eq!(
        store
            .execute_authenticated_import(
                "authenticated-after-unauthenticated",
                &snapshot,
                committed,
            )
            .unwrap_err(),
        DurableError::InvalidAudit(
            "authenticated inbound decision requires an authenticated accepted observation".into()
        )
    );
    let inspection = inspect_read_only(&path, &configuration, "r3").unwrap();
    assert_eq!(inspection.history_count, 0);
    assert_eq!(inspection.receipt_count, 0);
    assert_eq!(inspection.audit_event_count, 1);
}

#[test]
fn malformed_import_audit_is_rejected_before_an_independently_refused_snapshot() {
    let lab = Lab::new();
    lab.run(
        "r1",
        &observation(1, "prevalidate-source", "peer fact", false),
    );
    let mut snapshot = lab.snapshot("r1");
    snapshot.configuration.logical_manager_id = "foreign-manager".into();
    let configuration = store_configuration(&lab);
    let path = lab.database("r3");
    let mut store = Store::open(&path, configuration.clone(), "r3").unwrap();
    let mut observed = audit(
        "prevalidate-observed",
        "prevalidate-import",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("prevalidate-wire"),
    );
    observed.authenticated_peer_id = Some("r1".into());
    observed.peer_claim = Some("r1".into());
    store.record_exchange_audit(&observed).unwrap();
    let mut malformed = audit(
        "prevalidate-committed",
        "prevalidate-import",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("prevalidate-wire"),
    );
    malformed.authenticated_peer_id = Some("r1".into());
    malformed.peer_claim = Some("r1".into());
    malformed.request_frame_bytes = 1;
    assert_eq!(
        store
            .execute_authenticated_import("prevalidate-wire", &snapshot, malformed)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "inbound import phase cannot carry transferred bytes or frame intent".into()
        )
    );
    let inspection = inspect_read_only(&path, &configuration, "r3").unwrap();
    assert_eq!(inspection.history_count, 0);
    assert_eq!(inspection.receipt_count, 0);
    assert_eq!(inspection.audit_event_count, 1);
}

#[test]
fn inbound_attempt_cannot_mix_refused_and_accepted_decisions() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "dual-source", "peer fact", false));
    let snapshot = lab.snapshot("r1");
    let configuration = store_configuration(&lab);
    let path = lab.database("r3");
    let mut store = Store::open(&path, configuration.clone(), "r3").unwrap();
    let mut observed = audit(
        "dual-observed",
        "dual-decision",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("dual-wire"),
    );
    observed.authenticated_peer_id = Some("r1".into());
    observed.peer_claim = Some("r1".into());
    store.record_exchange_audit(&observed).unwrap();
    let mut refused = audit(
        "dual-refused",
        "dual-decision",
        AuditPhase::InboundRefusalRecorded,
        AuditOutcome::AuthenticatedRefusal,
        Some("dual-wire"),
    );
    refused.authenticated_peer_id = Some("r1".into());
    refused.peer_claim = Some("r1".into());
    store.record_exchange_audit(&refused).unwrap();
    let mut committed = audit(
        "dual-committed",
        "dual-decision",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("dual-wire"),
    );
    committed.authenticated_peer_id = Some("r1".into());
    committed.peer_claim = Some("r1".into());
    assert_eq!(
        store
            .execute_authenticated_import("dual-wire", &snapshot, committed)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "inbound exchange contains both accepted and refused decisions".into()
        )
    );
    let inspection = inspect_read_only(&path, &configuration, "r3").unwrap();
    assert_eq!(inspection.history_count, 0);
    assert_eq!(inspection.receipt_count, 0);
    assert_eq!(inspection.audit_event_count, 2);
}

#[test]
fn refusal_decision_requires_the_authenticated_observed_peer() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    assert_eq!(
        RefusalReason::TransportUnavailable.to_string(),
        "transport_unavailable"
    );
    assert_eq!(
        serde_json::to_string(&RefusalReason::TransportUnavailable).unwrap(),
        "\"transport_unavailable\""
    );
    let mut wrong_unavailable_reason = audit(
        "unavailable-unsafe-store",
        "unavailable-unsafe-store",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Unavailable,
        None,
    );
    wrong_unavailable_reason.reason_code = Some(RefusalReason::UnsafeStore);
    assert_eq!(
        store
            .record_exchange_audit(&wrong_unavailable_reason)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "unavailable evidence requires an unavailable category and transport_unavailable reason"
                .into()
        )
    );
    store
        .record_exchange_audit(&audit(
            "refusal-unauth-observed",
            "refusal-unauth",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::UnauthenticatedDiagnostic,
            Some("refusal-unauth-wire"),
        ))
        .unwrap();
    let refusal = audit(
        "refusal-after-unauth",
        "refusal-unauth",
        AuditPhase::InboundRefusalRecorded,
        AuditOutcome::AuthenticatedRefusal,
        Some("refusal-unauth-wire"),
    );
    assert_eq!(
        store.record_exchange_audit(&refusal).unwrap_err(),
        DurableError::InvalidAudit(
            "authenticated inbound decision requires an authenticated accepted observation".into()
        )
    );

    for (suffix, reason) in [
        ("unsafe-store", RefusalReason::UnsafeStore),
        ("transport-unavailable", RefusalReason::TransportUnavailable),
    ] {
        let mut local_failure = audit(
            &format!("refusal-local-{suffix}"),
            &format!("refusal-local-{suffix}"),
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
            Some(&format!("refusal-local-{suffix}-wire")),
        );
        local_failure.reason_code = Some(reason);
        assert_eq!(
            store.record_exchange_audit(&local_failure).unwrap_err(),
            DurableError::InvalidAudit(
                "authenticated refusal reason is not a peer-request refusal".into()
            )
        );
    }
}

#[test]
fn malformed_and_unauthenticated_diagnostics_require_invalid_request_reason() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    for outcome in [
        AuditOutcome::Malformed,
        AuditOutcome::UnauthenticatedDiagnostic,
    ] {
        for reason in [
            RefusalReason::PolicyViolation,
            RefusalReason::OperationIdReused,
            RefusalReason::UnsupportedSchema,
            RefusalReason::IdentityMismatch,
            RefusalReason::UnrecognizedStore,
            RefusalReason::MissingStore,
            RefusalReason::UnsafeStore,
            RefusalReason::TransportUnavailable,
        ] {
            let suffix = format!("{outcome:?}-{reason:?}").to_ascii_lowercase();
            let mut event = audit(
                &format!("diagnostic-reason-{suffix}"),
                &format!("diagnostic-reason-{suffix}"),
                AuditPhase::InboundRequestObserved,
                outcome,
                None,
            );
            event.reason_code = Some(reason);
            assert_eq!(
                store.record_exchange_audit(&event).unwrap_err(),
                DurableError::InvalidAudit(
                    "malformed or unauthenticated diagnostic evidence requires a malformed category and invalid_request reason"
                        .into()
                )
            );
        }
        for category in [AuditErrorCategory::Unavailable, AuditErrorCategory::Refused] {
            let suffix = format!("{outcome:?}-{category:?}").to_ascii_lowercase();
            let mut event = audit(
                &format!("diagnostic-category-{suffix}"),
                &format!("diagnostic-category-{suffix}"),
                AuditPhase::InboundRequestObserved,
                outcome,
                None,
            );
            event.error_category = Some(category);
            assert_eq!(
                store.record_exchange_audit(&event).unwrap_err(),
                DurableError::InvalidAudit(
                    "malformed or unauthenticated diagnostic evidence requires a malformed category and invalid_request reason"
                        .into()
                )
            );
        }
    }
}

#[test]
fn stored_diagnostic_with_local_store_reason_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    let mut event = audit(
        "stored-invalid-diagnostic-reason",
        "stored-invalid-diagnostic-reason",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Malformed,
        None,
    );
    store.record_exchange_audit(&event).unwrap();
    drop(store);

    event.reason_code = Some(RefusalReason::IdentityMismatch);
    let record_json = serde_json::to_string(&event).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET reason_code='identity_mismatch',
             record_json=?1, sha256=?2
             WHERE audit_event_id='stored-invalid-diagnostic-reason'",
            params![record_json, audit_checksum(&record_json)],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);

    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "stored audit is invalid: invalid_audit: malformed or unauthenticated diagnostic evidence requires a malformed category and invalid_request reason"
                .into()
        )
    );
}

#[test]
fn stored_diagnostic_with_wrong_category_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    let mut event = audit(
        "stored-invalid-diagnostic-category",
        "stored-invalid-diagnostic-category",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    store.record_exchange_audit(&event).unwrap();
    drop(store);

    event.error_category = Some(AuditErrorCategory::Refused);
    let record_json = serde_json::to_string(&event).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET error_category='refused',
             record_json=?1, sha256=?2
             WHERE audit_event_id='stored-invalid-diagnostic-category'",
            params![record_json, audit_checksum(&record_json)],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);

    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "stored audit is invalid: invalid_audit: malformed or unauthenticated diagnostic evidence requires a malformed category and invalid_request reason"
                .into()
        )
    );
}

#[test]
fn complete_accepted_inbound_chain_closes_only_after_verified_reply_write() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "accepted-source", "peer fact", false));
    let snapshot = lab.snapshot("r1");
    let configuration = store_configuration(&lab);
    let path = lab.database("r2");
    let mut store = Store::open(&path, configuration.clone(), "r2").unwrap();
    let mut observed = audit(
        "accepted-observed",
        "accepted-chain",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("accepted-wire"),
    );
    observed.authenticated_peer_id = Some("r1".into());
    observed.peer_claim = Some("r1".into());
    store.record_exchange_audit(&observed).unwrap();
    let mut committed = audit(
        "accepted-committed",
        "accepted-chain",
        AuditPhase::InboundImportCommitted,
        AuditOutcome::Accepted,
        Some("accepted-wire"),
    );
    committed.authenticated_peer_id = Some("r1".into());
    committed.peer_claim = Some("r1".into());
    let imported = store
        .execute_authenticated_import("accepted-wire", &snapshot, committed)
        .unwrap();
    let receipt = imported.executed.receipt.unwrap();
    let mut prepared = audit(
        "accepted-reply-prepared",
        "accepted-chain",
        AuditPhase::InboundReplyPrepared,
        AuditOutcome::Incomplete,
        Some("accepted-wire"),
    );
    prepared.authenticated_peer_id = Some("r1".into());
    prepared.peer_claim = Some("r1".into());
    prepared.local_receipt_operation_id = Some(receipt.operation_id.clone());
    prepared.local_receipt_sha256 = Some(receipt.sha256.clone());
    let mut wrong_prepared_receipt = prepared.clone();
    wrong_prepared_receipt.audit_event_id = "accepted-reply-wrong-prepared-receipt".into();
    wrong_prepared_receipt.local_receipt_operation_id = Some("network:wrong".into());
    assert_eq!(
        store
            .record_exchange_audit(&wrong_prepared_receipt)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "inbound reply preparation does not match its durable decision".into()
        )
    );
    store.record_exchange_audit(&prepared).unwrap();
    let mut written = audit(
        "accepted-reply-written",
        "accepted-chain",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::Accepted,
        Some("accepted-wire"),
    );
    written.authenticated_peer_id = Some("r1".into());
    written.peer_claim = Some("r1".into());
    written.local_receipt_operation_id = Some(receipt.operation_id);
    written.local_receipt_sha256 = Some(receipt.sha256);
    let mut wrong_written_receipt = written.clone();
    wrong_written_receipt.audit_event_id = "accepted-reply-wrong-written-receipt".into();
    wrong_written_receipt.local_receipt_operation_id = Some("network:wrong".into());
    assert_eq!(
        store
            .record_exchange_audit(&wrong_written_receipt)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "inbound reply write does not match its prepared decision".into()
        )
    );
    store.record_exchange_audit(&written).unwrap();
    assert!(inspect_read_only(&path, &configuration, "r2")
        .unwrap()
        .incomplete_attempts
        .is_empty());
}

#[test]
fn complete_refusal_and_no_reply_chains_preserve_their_decisions() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    let observed = audit(
        "refused-observed",
        "refused-chain",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("refused-wire"),
    );
    store.record_exchange_audit(&observed).unwrap();
    let refused = audit(
        "refusal-recorded",
        "refused-chain",
        AuditPhase::InboundRefusalRecorded,
        AuditOutcome::AuthenticatedRefusal,
        Some("refused-wire"),
    );
    store.record_exchange_audit(&refused).unwrap();
    let prepared = audit(
        "refused-reply-prepared",
        "refused-chain",
        AuditPhase::InboundReplyPrepared,
        AuditOutcome::Incomplete,
        Some("refused-wire"),
    );
    store.record_exchange_audit(&prepared).unwrap();
    let written = audit(
        "refused-reply-written",
        "refused-chain",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::AuthenticatedRefusal,
        Some("refused-wire"),
    );
    let mut wrong_reason = written.clone();
    wrong_reason.audit_event_id = "refused-reply-wrong-reason".into();
    wrong_reason.reason_code = Some(RefusalReason::InvalidRequest);
    assert_eq!(
        store.record_exchange_audit(&wrong_reason).unwrap_err(),
        DurableError::InvalidAudit(
            "inbound reply outcome does not match its durable decision".into()
        )
    );
    let mut wrong_frame = written.clone();
    wrong_frame.audit_event_id = "refused-reply-wrong-frame".into();
    wrong_frame.reply_sha256 = Some(format!("{:064x}", 7));
    assert_eq!(
        store.record_exchange_audit(&wrong_frame).unwrap_err(),
        DurableError::InvalidAudit("inbound reply write does not match its prepared frame".into())
    );
    store.record_exchange_audit(&written).unwrap();

    let diagnostic = audit(
        "diagnostic-observed",
        "diagnostic-close",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    store.record_exchange_audit(&diagnostic).unwrap();
    let closed = audit(
        "diagnostic-closed",
        "diagnostic-close",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    store.record_exchange_audit(&closed).unwrap();
    assert!(inspect_read_only(&path, &configuration, "r1")
        .unwrap()
        .incomplete_attempts
        .is_empty());
}

#[test]
fn diagnostic_replies_are_terminal_only_without_a_signed_decision() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    let observed = audit(
        "diagnostic-written-observed",
        "diagnostic-written",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    store.record_exchange_audit(&observed).unwrap();
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1")
            .unwrap()
            .incomplete_attempts
            .len(),
        1
    );
    let diagnostic = audit(
        "diagnostic-written-terminal",
        "diagnostic-written",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    store.record_exchange_audit(&diagnostic).unwrap();

    let authenticated = audit(
        "authenticated-unavailable-observed",
        "authenticated-unavailable",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("authenticated-unavailable-wire"),
    );
    store.record_exchange_audit(&authenticated).unwrap();
    let mut unavailable = audit(
        "authenticated-unavailable-diagnostic",
        "authenticated-unavailable",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        Some("authenticated-unavailable-wire"),
    );
    unavailable.authenticated_peer_id = Some("r2".into());
    store.record_exchange_audit(&unavailable).unwrap();

    let authenticated_partial = audit(
        "authenticated-partial-observed",
        "authenticated-partial",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("authenticated-partial-wire"),
    );
    store.record_exchange_audit(&authenticated_partial).unwrap();
    let mut partial_unavailable = audit(
        "authenticated-partial-diagnostic",
        "authenticated-partial",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        Some("authenticated-partial-wire"),
    );
    partial_unavailable.authenticated_peer_id = Some("r2".into());
    partial_unavailable.reply_frame_bytes = 4;
    store.record_exchange_audit(&partial_unavailable).unwrap();
    assert!(inspect_read_only(&path, &configuration, "r1")
        .unwrap()
        .incomplete_attempts
        .is_empty());
}

#[test]
fn partial_diagnostic_reply_is_terminal_only_as_unavailable() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    for bytes in [1, 3, 4, 67] {
        let attempt = format!("partial-diagnostic-{bytes}");
        let observed = audit(
            &format!("{attempt}-observed"),
            &attempt,
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Malformed,
            None,
        );
        store.record_exchange_audit(&observed).unwrap();
        let mut partial = audit(
            &format!("{attempt}-written"),
            &attempt,
            AuditPhase::InboundDiagnosticReplyWritten,
            AuditOutcome::Unavailable,
            None,
        );
        partial.reply_frame_bytes = bytes;
        store.record_exchange_audit(&partial).unwrap();
    }
    assert!(inspect_read_only(&path, &configuration, "r1")
        .unwrap()
        .incomplete_attempts
        .is_empty());
}

#[test]
fn partial_diagnostic_reply_requires_unavailable_outcome_and_category() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    for (event_id, outcome) in [
        ("partial-malformed", AuditOutcome::Malformed),
        (
            "partial-unauthenticated",
            AuditOutcome::UnauthenticatedDiagnostic,
        ),
    ] {
        let mut partial = audit(
            event_id,
            event_id,
            AuditPhase::InboundDiagnosticReplyWritten,
            outcome,
            None,
        );
        partial.reply_frame_bytes = 4;
        assert_eq!(
            store.record_exchange_audit(&partial).unwrap_err(),
            DurableError::InvalidAudit(
                "partial diagnostic reply write must record unavailability".into()
            )
        );
    }

    store
        .record_exchange_audit(&audit(
            "partial-unauth-peer-observed",
            "partial-unauth-peer",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Malformed,
            None,
        ))
        .unwrap();
    let mut asserted_peer = audit(
        "partial-unauth-peer-written",
        "partial-unauth-peer",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        None,
    );
    asserted_peer.reply_frame_bytes = 4;
    asserted_peer.authenticated_peer_id = Some("r2".into());
    assert_eq!(
        store.record_exchange_audit(&asserted_peer).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic after unauthenticated observation cannot assert an authenticated peer"
                .into()
        )
    );
}

#[test]
fn diagnostic_reply_requires_nonzero_reply_only_transfer_evidence() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();

    let mut zero = audit(
        "diagnostic-zero-bytes",
        "diagnostic-zero-bytes",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        None,
    );
    zero.reply_frame_bytes = 0;
    assert_eq!(
        store.record_exchange_audit(&zero).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply write requires bounded nonzero reply evidence".into()
        )
    );

    let mut missing_size = audit(
        "diagnostic-missing-reply-size",
        "diagnostic-missing-reply-size",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        None,
    );
    missing_size.reply_frame_bytes = 1;
    missing_size.reply_announced_body_bytes = None;
    assert_eq!(
        store.record_exchange_audit(&missing_size).unwrap_err(),
        DurableError::InvalidAudit(
            "outbound diagnostic reply frame requires its announced body size".into()
        )
    );

    let mut missing_digest = audit(
        "diagnostic-missing-reply-digest",
        "diagnostic-missing-reply-digest",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        None,
    );
    missing_digest.reply_frame_bytes = 1;
    missing_digest.reply_sha256 = None;
    assert_eq!(
        store.record_exchange_audit(&missing_digest).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply write requires bounded nonzero reply evidence".into()
        )
    );

    for (suffix, frame_bytes, announced_size, digest) in [
        ("bytes", 1, None, None),
        ("announced-size", 0, Some(128), None),
        ("digest", 0, None, Some(format!("{:064x}", 1))),
    ] {
        let mut repeated_request = audit(
            &format!("diagnostic-repeated-request-{suffix}"),
            &format!("diagnostic-repeated-request-{suffix}"),
            AuditPhase::InboundDiagnosticReplyWritten,
            AuditOutcome::Malformed,
            None,
        );
        repeated_request.request_frame_bytes = frame_bytes;
        repeated_request.request_announced_body_bytes = announced_size;
        repeated_request.request_sha256 = digest;
        assert_eq!(
            store.record_exchange_audit(&repeated_request).unwrap_err(),
            DurableError::InvalidAudit(
                "inbound diagnostic reply cannot repeat request transfer evidence".into()
            )
        );
    }
}

#[test]
fn diagnostic_reply_requires_the_observed_peer_reference() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();

    for (suffix, bytes, authenticated_peer_id, peer_claim) in [
        ("partial-missing", 4, None, Some("r2")),
        ("partial-changed", 4, Some("r3"), Some("r3")),
        ("full-missing", 68, None, Some("r2")),
        ("full-changed", 68, Some("r3"), Some("r3")),
    ] {
        let attempt = format!("authenticated-peer-{suffix}");
        store
            .record_exchange_audit(&audit(
                &format!("{attempt}-observed"),
                &attempt,
                AuditPhase::InboundRequestObserved,
                AuditOutcome::Accepted,
                Some(&format!("{attempt}-wire")),
            ))
            .unwrap();
        let mut diagnostic = audit(
            &format!("{attempt}-written"),
            &attempt,
            AuditPhase::InboundDiagnosticReplyWritten,
            AuditOutcome::Unavailable,
            Some(&format!("{attempt}-wire")),
        );
        diagnostic.reply_frame_bytes = bytes;
        diagnostic.authenticated_peer_id = authenticated_peer_id.map(str::to_string);
        diagnostic.peer_claim = peer_claim.map(str::to_string);
        assert_eq!(
            store.record_exchange_audit(&diagnostic).unwrap_err(),
            DurableError::InvalidAudit(
                "diagnostic after authenticated observation must record matching unavailability"
                    .into()
            )
        );
    }

    for (suffix, peer_claim) in [("missing", None), ("changed", Some("r3"))] {
        let attempt = format!("unauthenticated-peer-{suffix}");
        store
            .record_exchange_audit(&audit(
                &format!("{attempt}-observed"),
                &attempt,
                AuditPhase::InboundRequestObserved,
                AuditOutcome::Malformed,
                None,
            ))
            .unwrap();
        let mut diagnostic = audit(
            &format!("{attempt}-written"),
            &attempt,
            AuditPhase::InboundDiagnosticReplyWritten,
            AuditOutcome::Unavailable,
            None,
        );
        diagnostic.reply_frame_bytes = 4;
        diagnostic.peer_claim = peer_claim.map(str::to_string);
        assert_eq!(
            store.record_exchange_audit(&diagnostic).unwrap_err(),
            DurableError::InvalidAudit(
                "diagnostic reply does not match its observed diagnostic".into()
            )
        );
    }
}

#[test]
fn full_diagnostic_reply_cannot_claim_partial_write_unavailability() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "full-unavailable-observed",
            "full-unavailable",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Malformed,
            None,
        ))
        .unwrap();
    let full = audit(
        "full-unavailable-written",
        "full-unavailable",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        None,
    );
    assert_eq!(
        store.record_exchange_audit(&full).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply does not match its observed diagnostic".into()
        )
    );
}

fn assert_partial_diagnostic_rejects_decision_and_write(
    store: &mut Store,
    after_prepared: &ExchangeAuditEvent,
) {
    let written = audit(
        "diagnostic-conflict-written",
        "diagnostic-signed-conflict",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::AuthenticatedRefusal,
        Some("diagnostic-conflict-wire"),
    );
    store.record_exchange_audit(&written).unwrap();
    let mut after_write = after_prepared.clone();
    after_write.audit_event_id = "diagnostic-after-signed-write".into();
    assert_eq!(
        store.record_exchange_audit(&after_write).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply cannot follow a durable or prepared signed decision".into()
        )
    );

    for (event_id, phase, outcome) in [
        (
            "diagnostic-decision-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "diagnostic-decision-refused",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "diagnostic-after-decision",
                phase,
                outcome,
                Some("diagnostic-decision-wire"),
            ))
            .unwrap();
    }
    let mut after_decision = audit(
        "diagnostic-after-decision-written",
        "diagnostic-after-decision",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        Some("diagnostic-decision-wire"),
    );
    after_decision.authenticated_peer_id = Some("r2".into());
    after_decision.reply_frame_bytes = 4;
    assert_eq!(
        store.record_exchange_audit(&after_decision).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply cannot follow a durable or prepared signed decision".into()
        )
    );
}

#[test]
fn diagnostic_reply_rejects_signed_or_conflicting_terminal_chains() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    for (event_id, phase, outcome) in [
        (
            "diagnostic-conflict-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "diagnostic-conflict-refused",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
        (
            "diagnostic-conflict-prepared",
            AuditPhase::InboundReplyPrepared,
            AuditOutcome::Incomplete,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "diagnostic-signed-conflict",
                phase,
                outcome,
                Some("diagnostic-conflict-wire"),
            ))
            .unwrap();
    }
    let mut after_prepared = audit(
        "diagnostic-after-prepared",
        "diagnostic-signed-conflict",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        Some("diagnostic-conflict-wire"),
    );
    after_prepared.authenticated_peer_id = Some("r2".into());
    after_prepared.reply_frame_bytes = 4;
    assert_eq!(
        store.record_exchange_audit(&after_prepared).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply cannot follow a durable or prepared signed decision".into()
        )
    );
    assert_partial_diagnostic_rejects_decision_and_write(&mut store, &after_prepared);

    let observed = audit(
        "diagnostic-close-observed",
        "diagnostic-close-conflict",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Malformed,
        None,
    );
    store.record_exchange_audit(&observed).unwrap();
    let closed = audit(
        "diagnostic-close-terminal",
        "diagnostic-close-conflict",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::Malformed,
        None,
    );
    store.record_exchange_audit(&closed).unwrap();
    let diagnostic = audit(
        "diagnostic-after-close",
        "diagnostic-close-conflict",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Malformed,
        None,
    );
    assert_eq!(
        store.record_exchange_audit(&diagnostic).unwrap_err(),
        DurableError::InvalidAudit(
            "an inbound attempt cannot both write a diagnostic and close".into()
        )
    );

    let observed = audit(
        "diagnostic-mismatch-observed",
        "diagnostic-mismatch",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Malformed,
        None,
    );
    store.record_exchange_audit(&observed).unwrap();
    let mismatch = audit(
        "diagnostic-mismatch-written",
        "diagnostic-mismatch",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    assert_eq!(
        store.record_exchange_audit(&mismatch).unwrap_err(),
        DurableError::InvalidAudit(
            "diagnostic reply does not match its observed diagnostic".into()
        )
    );
}

#[test]
fn signed_reply_requires_matching_decision_preparation_and_peer() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    for (event_id, phase, outcome) in [
        (
            "write-missing-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "write-missing-refusal",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "write-missing-prepared",
                phase,
                outcome,
                Some("write-missing-wire"),
            ))
            .unwrap();
    }
    let written = audit(
        "write-without-prepared",
        "write-missing-prepared",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::AuthenticatedRefusal,
        Some("write-missing-wire"),
    );
    assert_eq!(
        store.record_exchange_audit(&written).unwrap_err(),
        DurableError::InvalidAudit("inbound reply write lacks its prepared predecessor".into())
    );

    for (event_id, phase, outcome) in [
        (
            "peer-mismatch-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "peer-mismatch-refusal",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "peer-mismatch",
                phase,
                outcome,
                Some("peer-mismatch-wire"),
            ))
            .unwrap();
    }
    let mut wrong_prepared_peer = audit(
        "peer-mismatch-prepared",
        "peer-mismatch",
        AuditPhase::InboundReplyPrepared,
        AuditOutcome::Incomplete,
        Some("peer-mismatch-wire"),
    );
    wrong_prepared_peer.authenticated_peer_id = Some("r3".into());
    wrong_prepared_peer.peer_claim = Some("r3".into());
    assert_eq!(
        store
            .record_exchange_audit(&wrong_prepared_peer)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "inbound reply preparation does not match its durable decision".into()
        )
    );
    let prepared = audit(
        "peer-match-prepared",
        "peer-mismatch",
        AuditPhase::InboundReplyPrepared,
        AuditOutcome::Incomplete,
        Some("peer-mismatch-wire"),
    );
    store.record_exchange_audit(&prepared).unwrap();
    let mut wrong_written_peer = audit(
        "peer-mismatch-written",
        "peer-mismatch",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::AuthenticatedRefusal,
        Some("peer-mismatch-wire"),
    );
    wrong_written_peer.authenticated_peer_id = Some("r3".into());
    wrong_written_peer.peer_claim = Some("r3".into());
    assert_eq!(
        store
            .record_exchange_audit(&wrong_written_peer)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "inbound reply write does not match its prepared decision".into()
        )
    );
}

#[test]
fn no_reply_close_rejects_decisions_mismatches_and_written_replies() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    for (event_id, phase, outcome) in [
        (
            "close-decision-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "close-decision-refused",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "close-with-decision",
                phase,
                outcome,
                Some("close-decision-wire"),
            ))
            .unwrap();
    }
    let mut close = audit(
        "close-before-prepared",
        "close-with-decision",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::Unavailable,
        Some("close-decision-wire"),
    );
    close.authenticated_peer_id = Some("r2".into());
    assert_eq!(
        store.record_exchange_audit(&close).unwrap_err(),
        DurableError::InvalidAudit(
            "a durable inbound decision must prepare a reply before close".into()
        )
    );

    store
        .record_exchange_audit(&audit(
            "close-decision-prepared",
            "close-with-decision",
            AuditPhase::InboundReplyPrepared,
            AuditOutcome::Incomplete,
            Some("close-decision-wire"),
        ))
        .unwrap();
    let mut unauthenticated_close = close.clone();
    unauthenticated_close.audit_event_id = "close-dropped-authentication".into();
    unauthenticated_close.authenticated_peer_id = None;
    assert_eq!(
        store
            .record_exchange_audit(&unauthenticated_close)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "close after reply preparation must record matching unavailability".into()
        )
    );

    let mut wrong_close_outcome = close;
    wrong_close_outcome.audit_event_id = "close-wrong-outcome".into();
    wrong_close_outcome.outcome = AuditOutcome::Malformed;
    wrong_close_outcome.error_category = Some(AuditErrorCategory::Malformed);
    wrong_close_outcome.reason_code = Some(RefusalReason::InvalidRequest);
    assert_eq!(
        store
            .record_exchange_audit(&wrong_close_outcome)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "close after reply preparation must record matching unavailability".into()
        )
    );
}

#[test]
fn no_reply_close_matches_observation_and_excludes_a_written_reply() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "close-auth-observed",
            "close-auth",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("close-auth-wire"),
        ))
        .unwrap();
    let close = audit(
        "close-auth-terminal",
        "close-auth",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::Unavailable,
        Some("close-auth-wire"),
    );
    assert_eq!(
        store.record_exchange_audit(&close).unwrap_err(),
        DurableError::InvalidAudit(
            "close after authenticated observation must record matching unavailability".into()
        )
    );

    store
        .record_exchange_audit(&audit(
            "close-unauth-observed",
            "close-unauth",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Malformed,
            None,
        ))
        .unwrap();
    let mismatch = audit(
        "close-unauth-terminal",
        "close-unauth",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::UnauthenticatedDiagnostic,
        None,
    );
    assert_eq!(
        store.record_exchange_audit(&mismatch).unwrap_err(),
        DurableError::InvalidAudit(
            "unauthenticated close does not match its observed diagnostic".into()
        )
    );

    for (event_id, phase, outcome) in [
        (
            "close-written-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "close-written-refused",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
        (
            "close-written-prepared",
            AuditPhase::InboundReplyPrepared,
            AuditOutcome::Incomplete,
        ),
        (
            "close-written-reply",
            AuditPhase::InboundReplyWriteObserved,
            AuditOutcome::AuthenticatedRefusal,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "close-after-write",
                phase,
                outcome,
                Some("close-written-wire"),
            ))
            .unwrap();
    }
    let mut after_write = audit(
        "close-after-written-reply",
        "close-after-write",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::Unavailable,
        Some("close-written-wire"),
    );
    after_write.authenticated_peer_id = Some("r2".into());
    assert_eq!(
        store.record_exchange_audit(&after_write).unwrap_err(),
        DurableError::InvalidAudit(
            "an inbound attempt cannot both write a reply and close without one".into()
        )
    );
}

#[test]
fn refusal_record_survives_restart_as_a_typed_incomplete_attempt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "crash-refusal-observed",
            "crash-after-refusal",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("crash-refusal-wire"),
        ))
        .unwrap();
    let refusal = audit(
        "crash-refusal-recorded",
        "crash-after-refusal",
        AuditPhase::InboundRefusalRecorded,
        AuditOutcome::AuthenticatedRefusal,
        Some("crash-refusal-wire"),
    );
    store.record_exchange_audit(&refusal).unwrap();
    drop(store);
    let inspection = inspect_read_only(&path, &configuration, "r1").unwrap();
    assert_eq!(inspection.incomplete_attempts.len(), 1);
    assert_eq!(
        inspection.incomplete_attempts[0].last_phase,
        AuditPhase::InboundRefusalRecorded
    );
}

#[test]
fn unauthenticated_observation_cannot_prepare_an_authenticated_decision_reply() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration, "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "unsafe-observed",
            "unsafe-promotion",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::UnauthenticatedDiagnostic,
            Some("unsafe-wire"),
        ))
        .unwrap();
    let mut prepared = audit(
        "unsafe-prepared",
        "unsafe-promotion",
        AuditPhase::InboundReplyPrepared,
        AuditOutcome::Incomplete,
        Some("unsafe-wire"),
    );
    prepared.authenticated_peer_id = None;
    assert_eq!(
        store.record_exchange_audit(&prepared).unwrap_err(),
        DurableError::InvalidAudit("inbound reply preparation lacks a durable decision".into())
    );
}

#[test]
fn stored_unauthenticated_promotion_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "stored-unsafe-observed",
            "stored-unsafe-promotion",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::UnauthenticatedDiagnostic,
            Some("stored-unsafe-wire"),
        ))
        .unwrap();
    let closed = audit(
        "stored-unsafe-next",
        "stored-unsafe-promotion",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::UnauthenticatedDiagnostic,
        Some("stored-unsafe-wire"),
    );
    store.record_exchange_audit(&closed).unwrap();
    drop(store);

    let mut promoted = audit(
        "stored-unsafe-next",
        "stored-unsafe-promotion",
        AuditPhase::InboundReplyPrepared,
        AuditOutcome::Incomplete,
        Some("stored-unsafe-wire"),
    );
    promoted.authenticated_peer_id = None;
    let record_json = serde_json::to_string(&promoted).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET phase='inbound_reply_prepared',
             authenticated_peer_id=NULL, request_frame_bytes=0,
             request_announced_body_bytes=NULL, request_sha256=NULL,
             reply_frame_bytes=0, reply_announced_body_bytes=64,
             reply_sha256=?1, outcome='incomplete', error_category=NULL,
             reason_code=NULL, record_json=?2, sha256=?3
             WHERE audit_event_id='stored-unsafe-next'",
            params![
                promoted.reply_sha256,
                record_json,
                audit_checksum(&record_json)
            ],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt("inbound reply preparation lacks a durable decision".into())
    );
}

#[test]
fn stored_diagnostic_promotion_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "stored-diagnostic-observed",
            "stored-diagnostic-promotion",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::UnauthenticatedDiagnostic,
            None,
        ))
        .unwrap();
    store
        .record_exchange_audit(&audit(
            "stored-diagnostic-written",
            "stored-diagnostic-promotion",
            AuditPhase::InboundDiagnosticReplyWritten,
            AuditOutcome::UnauthenticatedDiagnostic,
            None,
        ))
        .unwrap();
    drop(store);

    let promoted = audit(
        "stored-diagnostic-observed",
        "stored-diagnostic-promotion",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        None,
    );
    let record_json = serde_json::to_string(&promoted).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET authenticated_peer_id='r2', peer_claim='r2',
             outcome='accepted', error_category=NULL, reason_code=NULL,
             record_json=?1, sha256=?2 WHERE audit_event_id='stored-diagnostic-observed'",
            params![record_json, audit_checksum(&record_json)],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "diagnostic after authenticated observation must record matching unavailability".into()
        )
    );
}

#[test]
fn stored_authenticated_diagnostic_peer_change_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "stored-peer-observed",
            "stored-peer-change",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("stored-peer-wire"),
        ))
        .unwrap();
    let mut diagnostic = audit(
        "stored-peer-diagnostic",
        "stored-peer-change",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        Some("stored-peer-wire"),
    );
    diagnostic.authenticated_peer_id = Some("r2".into());
    diagnostic.reply_frame_bytes = 4;
    store.record_exchange_audit(&diagnostic).unwrap();
    drop(store);

    diagnostic.authenticated_peer_id = Some("r3".into());
    diagnostic.peer_claim = Some("r3".into());
    let record_json = serde_json::to_string(&diagnostic).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET authenticated_peer_id='r3', peer_claim='r3',
             record_json=?1, sha256=?2 WHERE audit_event_id='stored-peer-diagnostic'",
            params![record_json, audit_checksum(&record_json)],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "diagnostic after authenticated observation must record matching unavailability".into()
        )
    );
}

#[test]
fn stored_partial_diagnostic_with_non_unavailable_outcome_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "stored-partial-observed",
            "stored-partial",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Malformed,
            None,
        ))
        .unwrap();
    let complete = audit(
        "stored-partial-written",
        "stored-partial",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Malformed,
        None,
    );
    store.record_exchange_audit(&complete).unwrap();
    drop(store);

    let mut partial = complete;
    partial.reply_frame_bytes = 4;
    let record_json = serde_json::to_string(&partial).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET reply_frame_bytes=4, record_json=?1, sha256=?2
             WHERE audit_event_id='stored-partial-written'",
            params![record_json, audit_checksum(&record_json)],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "stored audit is invalid: invalid_audit: partial diagnostic reply write must record unavailability"
                .into()
        )
    );
}

#[test]
fn stored_diagnostic_after_prepared_signed_reply_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    for (event_id, phase, outcome) in [
        (
            "stored-signed-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "stored-signed-refused",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
        (
            "stored-signed-prepared",
            AuditPhase::InboundReplyPrepared,
            AuditOutcome::Incomplete,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "stored-signed-diagnostic",
                phase,
                outcome,
                Some("stored-signed-wire"),
            ))
            .unwrap();
    }
    let mut close = audit(
        "stored-signed-terminal",
        "stored-signed-diagnostic",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::Unavailable,
        Some("stored-signed-wire"),
    );
    close.authenticated_peer_id = Some("r2".into());
    store.record_exchange_audit(&close).unwrap();
    drop(store);

    let mut diagnostic = audit(
        "stored-signed-terminal",
        "stored-signed-diagnostic",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Unavailable,
        Some("stored-signed-wire"),
    );
    diagnostic.authenticated_peer_id = Some("r2".into());
    diagnostic.reply_frame_bytes = 4;
    let record_json = serde_json::to_string(&diagnostic).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET phase='inbound_diagnostic_reply_written',
             reply_frame_bytes=?1, reply_announced_body_bytes=?2, reply_sha256=?3,
             record_json=?4, sha256=?5 WHERE audit_event_id='stored-signed-terminal'",
            params![
                diagnostic.reply_frame_bytes,
                diagnostic.reply_announced_body_bytes,
                diagnostic.reply_sha256,
                record_json,
                audit_checksum(&record_json)
            ],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "diagnostic reply cannot follow a durable or prepared signed decision".into()
        )
    );
}

#[test]
fn stored_close_that_drops_authenticated_peer_is_corrupt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "stored-close-observed",
            "stored-close-auth",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("stored-close-wire"),
        ))
        .unwrap();
    let mut close = audit(
        "stored-close-terminal",
        "stored-close-auth",
        AuditPhase::InboundConnectionClosed,
        AuditOutcome::Unavailable,
        Some("stored-close-wire"),
    );
    close.authenticated_peer_id = Some("r2".into());
    store.record_exchange_audit(&close).unwrap();
    drop(store);

    close.authenticated_peer_id = None;
    let record_json = serde_json::to_string(&close).unwrap();
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET authenticated_peer_id=NULL,
             record_json=?1, sha256=?2 WHERE audit_event_id='stored-close-terminal'",
            params![record_json, audit_checksum(&record_json)],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    drop(database);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "close after authenticated observation must record matching unavailability".into()
        )
    );
}

#[test]
fn audit_rows_are_immutable_idempotent_bounded_and_fail_closed() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let database_path = lab.database("r1");
    let mut store = Store::open(&database_path, configuration.clone(), "r1").unwrap();
    let prepared = audit(
        "prepared-event",
        "attempt-open",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Incomplete,
        Some("sync-operation"),
    );
    let first = store.record_exchange_audit(&prepared).unwrap();
    assert_eq!(store.record_exchange_audit(&prepared).unwrap(), first);
    let mut changed = prepared.clone();
    changed.request_frame_bytes += 1;
    assert!(store.record_exchange_audit(&changed).is_err());
    let observed = store
        .execute_with_receipt(&observation(1, "audit-link-observe", "value", false))
        .unwrap();
    let receipt = observed.receipt.unwrap();
    let mut wrong_kind = audit(
        "wrong-kind-reference",
        "wrong-kind-attempt",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Incomplete,
        Some("sync-operation"),
    );
    wrong_kind.local_receipt_operation_id = Some(receipt.operation_id);
    wrong_kind.local_receipt_sha256 = Some(receipt.sha256);
    assert_eq!(
        store.record_exchange_audit(&wrong_kind).unwrap_err(),
        DurableError::InvalidAudit("this audit phase cannot reference a local receipt".into())
    );
    let mut invalid = audit(
        "invalid-reference",
        "attempt-invalid",
        AuditPhase::OutboundExchangeCompleted,
        AuditOutcome::Accepted,
        Some("missing-operation"),
    );
    invalid.local_receipt_operation_id = Some("missing-operation".into());
    invalid.local_receipt_sha256 = Some(format!("{:064x}", 9));
    assert!(store.record_exchange_audit(&invalid).is_err());

    let inspection = inspect_read_only(&database_path, &configuration, "r1").unwrap();
    assert_eq!(inspection.audit_event_count, 1);
    assert_eq!(
        inspection
            .incomplete_attempts
            .iter()
            .map(|item| item.attempt_id.as_str())
            .collect::<Vec<_>>(),
        [prepared.attempt_id.as_str()]
    );
    let database = Connection::open(&database_path).unwrap();
    assert!(database
        .execute(
            "UPDATE exchange_audit_events SET request_frame_bytes=999",
            []
        )
        .is_err());
    assert!(database
        .execute("DELETE FROM exchange_audit_events", [])
        .is_err());
    database
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    database
        .execute(
            "UPDATE exchange_audit_events SET request_frame_bytes=999",
            [],
        )
        .unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    assert_eq!(
        inspect_read_only(&database_path, &configuration, "r1").unwrap_err(),
        DurableError::Corrupt(
            "stored audit is invalid: invalid_audit: outbound request preparation records intent with zero transferred bytes"
                .into()
        )
    );
    assert!(store.execute(&Request::Inspect {}).is_err());
}

#[test]
fn schema_v2_is_refused_without_migration_or_mutation() {
    let lab = Lab::new();
    let path = lab.database("r1");
    let database = Connection::open(&path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), replica_id TEXT NOT NULL, topology_json TEXT NOT NULL);
             CREATE TABLE facts (event_id TEXT PRIMARY KEY, fact_json TEXT NOT NULL, sha256 TEXT NOT NULL);
             CREATE TABLE receipts (operation_id TEXT PRIMARY KEY, request_json TEXT NOT NULL, response_json TEXT NOT NULL, sha256 TEXT NOT NULL);
             PRAGMA user_version=2;",
        )
        .unwrap();
    drop(database);
    let before = fs::read(&path).unwrap();
    assert!(Store::open(&path, store_configuration(&lab), "r1").is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let database = Connection::open(&path).unwrap();
    assert_eq!(
        database
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        database
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        3
    );
}

#[test]
fn canonical_read_only_inspection_preserves_live_wal_and_rejects_missing_or_mismatched_store() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    store
        .execute_with_receipt(&observation(1, "live-write", "live", false))
        .unwrap();
    store
        .record_exchange_audit(&audit(
            "live-prepared",
            "live-attempt",
            AuditPhase::OutboundRequestPrepared,
            AuditOutcome::Incomplete,
            Some("live-sync"),
        ))
        .unwrap();
    assert!(
        fs::metadata(PathBuf::from(format!("{}-wal", path.display())))
            .unwrap()
            .len()
            > 0
    );
    let before = directory_state(lab.directory.path());
    let inspection = inspect_read_only(&path, &configuration, "r1").unwrap();
    assert_eq!(inspection.schema_version, 3);
    assert_eq!(inspection.logical_manager_id, "manager");
    assert_eq!(inspection.replica_id, "r1");
    assert_eq!(inspection.sqlite_integrity_result, "ok");
    assert_eq!(inspection.history_count, 1);
    assert_eq!(inspection.receipt_count, 1);
    assert_eq!(inspection.audit_event_count, 1);
    assert_eq!(before, directory_state(lab.directory.path()));

    assert!(inspect_read_only(&lab.database("missing"), &configuration, "r1").is_err());
    assert!(!lab.database("missing").exists());
    let mismatched_before = directory_state(lab.directory.path());
    assert!(inspect_read_only(&path, &configuration, "r2").is_err());
    assert_eq!(mismatched_before, directory_state(lab.directory.path()));
    assert_eq!(
        Store::open(&path, configuration.clone(), "r2")
            .err()
            .unwrap(),
        DurableError::Refused(RefusalReason::IdentityMismatch)
    );
    assert_eq!(mismatched_before, directory_state(lab.directory.path()));
}

#[test]
fn canonical_history_digest_matches_after_convergence_and_terminal_audit_closes_attempt() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    lab.run("r1", &observation(1, "r1-local", "one", false));
    lab.run("r2", &observation(2, "r2-local", "two", false));
    let snapshots = [lab.snapshot("r1"), lab.snapshot("r2")];
    for replica in ["r1", "r2", "r3"] {
        for snapshot in &snapshots {
            lab.import(
                replica,
                snapshot.clone(),
                &format!("converge-{replica}-{}", snapshot.replica_id),
            );
        }
    }
    let digests: Vec<_> = ["r1", "r2", "r3"]
        .into_iter()
        .map(|replica| {
            inspect_read_only(&lab.database(replica), &configuration, replica)
                .unwrap()
                .logical_history_sha256
        })
        .collect();
    assert_eq!(digests[0], digests[1]);
    assert_eq!(digests[1], digests[2]);
    let mut other_manager = configuration.clone();
    other_manager.logical_manager_id = "other-manager".into();
    let other_path = lab.directory.path().join("other-manager.sqlite");
    let empty_manager_path = lab.directory.path().join("empty-manager.sqlite");
    Store::open(&other_path, other_manager.clone(), "r1").unwrap();
    Store::open(&empty_manager_path, configuration.clone(), "r1").unwrap();
    assert_ne!(
        inspect_read_only(&other_path, &other_manager, "r1")
            .unwrap()
            .logical_history_sha256,
        inspect_read_only(&empty_manager_path, &configuration, "r1")
            .unwrap()
            .logical_history_sha256
    );

    let mut store = Store::open(&lab.database("r1"), configuration.clone(), "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "terminal-prepared",
            "terminal-attempt",
            AuditPhase::OutboundRequestPrepared,
            AuditOutcome::Incomplete,
            Some("terminal-operation"),
        ))
        .unwrap();
    let mut completed = audit(
        "terminal-completed",
        "terminal-attempt",
        AuditPhase::OutboundExchangeCompleted,
        AuditOutcome::Accepted,
        Some("terminal-operation"),
    );
    completed.remote_receipt_operation_id = Some(
        authenticated_import_receipt_id(
            &configuration.topology().unwrap(),
            "r1",
            "terminal-operation",
        )
        .unwrap(),
    );
    completed.remote_receipt_sha256 = Some(format!("{:064x}", 4));
    let mut wrong_remote = completed.clone();
    wrong_remote.audit_event_id = "terminal-wrong-remote".into();
    wrong_remote.remote_receipt_operation_id = Some("arbitrary-remote-receipt".into());
    assert_eq!(
        store.record_exchange_audit(&wrong_remote).unwrap_err(),
        DurableError::InvalidAudit(
            "remote receipt identity does not match the authenticated import mapping".into()
        )
    );
    let mut wrong_request = completed.clone();
    wrong_request.audit_event_id = "terminal-wrong-request".into();
    wrong_request.request_sha256 = Some(format!("{:064x}", 8));
    assert_eq!(
        store.record_exchange_audit(&wrong_request).unwrap_err(),
        DurableError::InvalidAudit(
            "outbound completion does not match its prepared request".into()
        )
    );
    store.record_exchange_audit(&completed).unwrap();
    let mut changed_replay = completed.clone();
    changed_replay.replayed = true;
    assert_eq!(
        store.record_exchange_audit(&changed_replay).unwrap_err(),
        DurableError::InvalidAudit("audit event ID reused with different bytes".into())
    );
    assert!(inspect_read_only(&lab.database("r1"), &configuration, "r1")
        .unwrap()
        .incomplete_attempts
        .is_empty());
}

#[test]
fn local_attempt_identity_is_distinct_from_reused_wire_nonce_and_incomplete_output_is_typed() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    let first_id = store.new_attempt_id("peer-reused-nonce").unwrap();
    let second_id = store.new_attempt_id("peer-reused-nonce").unwrap();
    assert_ne!(first_id, second_id);
    for (event_id, attempt_id, operation_id) in [
        ("observed-first", first_id.as_str(), "wire-operation-1"),
        ("observed-second", second_id.as_str(), "wire-operation-2"),
    ] {
        let mut event = audit(
            event_id,
            attempt_id,
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some(operation_id),
        );
        event.wire_nonce = "peer-reused-nonce".into();
        store.record_exchange_audit(&event).unwrap();
    }
    let mut duplicate_phase = audit(
        "observed-first-again",
        &first_id,
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("wire-operation-1"),
    );
    duplicate_phase.wire_nonce = "peer-reused-nonce".into();
    assert_eq!(
        store.record_exchange_audit(&duplicate_phase).unwrap_err(),
        DurableError::InvalidAudit("exchange attempt already contains this phase".into())
    );
    let inspection = inspect_read_only(&path, &configuration, "r1").unwrap();
    assert_eq!(inspection.incomplete_attempts.len(), 2);
    assert!(inspection
        .incomplete_attempts
        .windows(2)
        .all(|pair| pair[0] < pair[1]));
    assert!(inspection.incomplete_attempts.iter().all(|attempt| {
        attempt.direction == AuditDirection::Inbound
            && attempt.wire_nonce == "peer-reused-nonce"
            && attempt.last_phase == AuditPhase::InboundRequestObserved
    }));

    let terminal_without_predecessor = audit(
        "missing-predecessor",
        "locally-distinct-attempt",
        AuditPhase::OutboundExchangeCompleted,
        AuditOutcome::Unavailable,
        Some("wire-operation"),
    );
    assert_eq!(
        store
            .record_exchange_audit(&terminal_without_predecessor)
            .unwrap_err(),
        DurableError::InvalidAudit("outbound exchange lacks its prepared predecessor".into())
    );
    let invalid_prepared = audit(
        "terminal-prepared-outcome",
        "another-attempt",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Accepted,
        Some("wire-operation"),
    );
    assert_eq!(
        store.record_exchange_audit(&invalid_prepared).unwrap_err(),
        DurableError::InvalidAudit(
            "prepared phases require incomplete outcome and incomplete is reserved for prepared phases"
                .into()
        )
    );

    for invalid in [
        "peer-nonce".to_string(),
        "attempt:".to_string(),
        format!("attempt:{}", "A".repeat(64)),
    ] {
        let mut invalid_attempt = audit(
            "invalid-attempt-format",
            "valid-seed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("wire-operation"),
        );
        invalid_attempt.attempt_id = invalid;
        assert!(matches!(
            store.record_exchange_audit(&invalid_attempt),
            Err(DurableError::InvalidAudit(_))
        ));
    }
}

#[test]
fn preauthentication_nonce_records_diagnostic_terminals_without_authority() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let path = lab.database("r1");
    let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
    for (attempt, terminal_phase) in [
        (
            "preauth-diagnostic-attempt",
            AuditPhase::InboundDiagnosticReplyWritten,
        ),
        ("preauth-close-attempt", AuditPhase::InboundConnectionClosed),
    ] {
        store
            .record_exchange_audit(&preauth_audit(
                &format!("{attempt}-observed"),
                attempt,
                AuditPhase::InboundRequestObserved,
                AuditOutcome::Malformed,
            ))
            .unwrap();
        store
            .record_exchange_audit(&preauth_audit(
                &format!("{attempt}-terminal"),
                attempt,
                terminal_phase,
                AuditOutcome::Malformed,
            ))
            .unwrap();
    }
    let inspection = inspect_read_only(&path, &configuration, "r1").unwrap();
    assert!(inspection.incomplete_attempts.is_empty());
    assert!(inspection.ordered_audit_events.iter().all(|evidence| {
        let event = &evidence.event;
        event.wire_nonce.starts_with("preauth:")
            && event.authenticated_peer_id.is_none()
            && event.peer_claim.is_none()
            && event.operation_id.is_none()
            && event.local_receipt_operation_id.is_none()
            && event.remote_receipt_operation_id.is_none()
            && !event.replayed
    }));
}

#[test]
fn preauthentication_nonce_shape_authority_and_attempt_stability_are_enforced() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();

    let mut malformed = preauth_audit(
        "preauth-malformed",
        "preauth-malformed",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Malformed,
    );
    malformed.wire_nonce = "preauth:ABC".into();
    assert_eq!(
        store.record_exchange_audit(&malformed).unwrap_err(),
        DurableError::InvalidAudit(
            "pre-authentication wire nonce must use preauth:<sha256> format".into()
        )
    );
    assert_eq!(
        store.new_attempt_id("preauth:ABC").unwrap_err(),
        DurableError::Refused(RefusalReason::InvalidRequest)
    );
    assert!(store
        .new_attempt_id(&format!("preauth:{:064x}", 9))
        .unwrap()
        .starts_with("attempt:"));

    let mut authority = preauth_audit(
        "preauth-authority",
        "preauth-authority",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Malformed,
    );
    authority.peer_claim = Some("r2".into());
    assert_eq!(
        store.record_exchange_audit(&authority).unwrap_err(),
        DurableError::InvalidAudit(
            "pre-authentication wire nonce cannot carry peer, operation, or receipt authority"
                .into()
        )
    );

    let mut wrong_phase = audit(
        "preauth-outbound",
        "preauth-outbound",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Incomplete,
        None,
    );
    wrong_phase.wire_nonce = format!("preauth:{:064x}", 1);
    wrong_phase.peer_claim = None;
    assert_eq!(
        store.record_exchange_audit(&wrong_phase).unwrap_err(),
        DurableError::InvalidAudit(
            "pre-authentication wire nonce is limited to inbound observation and diagnostic terminals"
                .into()
        )
    );

    let observed = preauth_audit(
        "preauth-stable-observed",
        "preauth-stable",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Malformed,
    );
    store.record_exchange_audit(&observed).unwrap();
    let mut replaced = preauth_audit(
        "preauth-stable-terminal",
        "preauth-stable",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Malformed,
    );
    replaced.wire_nonce = "decoded-peer-nonce".into();
    assert_eq!(
        store.record_exchange_audit(&replaced).unwrap_err(),
        DurableError::InvalidAudit("inbound exchange phases do not describe one attempt".into())
    );
}

#[test]
fn audit_attempt_direction_metadata_and_inbound_predecessor_are_enforced() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    store
        .record_exchange_audit(&audit(
            "direction-outbound",
            "direction-conflict",
            AuditPhase::OutboundRequestPrepared,
            AuditOutcome::Incomplete,
            Some("direction-wire"),
        ))
        .unwrap();
    let inbound = audit(
        "direction-inbound",
        "direction-conflict",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("direction-wire"),
    );
    assert_eq!(
        store.record_exchange_audit(&inbound).unwrap_err(),
        DurableError::InvalidAudit("one attempt ID cannot span both audit directions".into())
    );

    let refusal = audit(
        "missing-observation-refusal",
        "missing-observation",
        AuditPhase::InboundRefusalRecorded,
        AuditOutcome::AuthenticatedRefusal,
        Some("missing-observation-wire"),
    );
    assert_eq!(
        store.record_exchange_audit(&refusal).unwrap_err(),
        DurableError::InvalidAudit("inbound exchange lacks its request observation".into())
    );

    store
        .record_exchange_audit(&audit(
            "metadata-observed",
            "metadata-mismatch",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("metadata-wire"),
        ))
        .unwrap();
    let decision = audit(
        "metadata-refused",
        "metadata-mismatch",
        AuditPhase::InboundRefusalRecorded,
        AuditOutcome::AuthenticatedRefusal,
        Some("different-wire-operation"),
    );
    assert_eq!(
        store.record_exchange_audit(&decision).unwrap_err(),
        DurableError::InvalidAudit("inbound exchange phases do not describe one attempt".into())
    );

    store
        .record_exchange_audit(&audit(
            "outbound-metadata-prepared",
            "outbound-metadata",
            AuditPhase::OutboundRequestPrepared,
            AuditOutcome::Incomplete,
            Some("outbound-wire"),
        ))
        .unwrap();
    let completed = audit(
        "outbound-metadata-completed",
        "outbound-metadata",
        AuditPhase::OutboundExchangeCompleted,
        AuditOutcome::Unavailable,
        Some("different-outbound-wire"),
    );
    assert_eq!(
        store.record_exchange_audit(&completed).unwrap_err(),
        DurableError::InvalidAudit("outbound exchange phases do not describe one attempt".into())
    );
}

#[test]
fn audit_byte_accounting_distinguishes_prepared_outbound_and_received_inbound_frames() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    let prepared = audit(
        "bytes-prepared",
        "bytes-prepared",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Incomplete,
        Some("bytes-operation"),
    );
    assert_eq!(prepared.request_frame_bytes, 0);
    assert_eq!(prepared.reply_frame_bytes, 0);
    store.record_exchange_audit(&prepared).unwrap();

    let mut partial_outbound = audit(
        "bytes-partial-outbound",
        "bytes-partial-outbound",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Incomplete,
        Some("bytes-operation-2"),
    );
    partial_outbound.request_frame_bytes = 1;
    assert_eq!(
        store.record_exchange_audit(&partial_outbound).unwrap_err(),
        DurableError::InvalidAudit(
            "outbound request preparation records intent with zero transferred bytes".into()
        )
    );

    let mut inbound_partial = audit(
        "bytes-inbound-partial",
        "bytes-inbound-partial",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("bytes-operation-3"),
    );
    inbound_partial.request_frame_bytes = 17;
    assert_eq!(
        store.record_exchange_audit(&inbound_partial).unwrap_err(),
        DurableError::InvalidAudit(
            "complete inbound request frame requires digest and exact byte accounting".into()
        )
    );
    let mut outbound_partial = audit(
        "bytes-outbound-partial-terminal",
        "bytes-outbound-partial-terminal",
        AuditPhase::OutboundExchangeCompleted,
        AuditOutcome::Unavailable,
        Some("bytes-operation-4"),
    );
    outbound_partial.request_frame_bytes = 17;
    let predecessor = audit(
        "bytes-outbound-partial-predecessor",
        "bytes-outbound-partial-terminal",
        AuditPhase::OutboundRequestPrepared,
        AuditOutcome::Incomplete,
        Some("bytes-operation-4"),
    );
    store.record_exchange_audit(&predecessor).unwrap();
    store.record_exchange_audit(&outbound_partial).unwrap();

    let mut incomplete_reply = audit(
        "bytes-incomplete-reply",
        "bytes-incomplete-reply",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::Accepted,
        Some("bytes-operation-5"),
    );
    incomplete_reply.reply_frame_bytes = 17;
    assert_eq!(
        store.record_exchange_audit(&incomplete_reply).unwrap_err(),
        DurableError::InvalidAudit(
            "complete outbound reply frame requires digest and exact byte accounting".into()
        )
    );
}

#[test]
fn accepted_requests_require_complete_frames_and_refusals_cannot_assert_receipts() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    let mut inbound_without_frame = audit(
        "bytes-inbound-without-frame",
        "bytes-inbound-without-frame",
        AuditPhase::InboundRequestObserved,
        AuditOutcome::Accepted,
        Some("bytes-operation-without-frame"),
    );
    inbound_without_frame.request_frame_bytes = 0;
    inbound_without_frame.request_announced_body_bytes = None;
    inbound_without_frame.request_sha256 = None;
    assert_eq!(
        store
            .record_exchange_audit(&inbound_without_frame)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "complete inbound request frame requires digest and exact byte accounting".into()
        )
    );

    let mut refusal_with_receipt = audit(
        "bytes-refusal-with-receipt",
        "bytes-refusal-with-receipt",
        AuditPhase::OutboundExchangeCompleted,
        AuditOutcome::AuthenticatedRefusal,
        Some("bytes-operation-refused"),
    );
    refusal_with_receipt.remote_receipt_operation_id = Some("network:refused".into());
    refusal_with_receipt.remote_receipt_sha256 = Some(format!("{:064x}", 3));
    assert_eq!(
        store
            .record_exchange_audit(&refusal_with_receipt)
            .unwrap_err(),
        DurableError::InvalidAudit(
            "only an accepted outbound completion can reference a remote receipt".into()
        )
    );
}

#[test]
fn reply_failure_uses_close_for_zero_bytes_and_write_for_partial_bytes_only() {
    let lab = Lab::new();
    let configuration = store_configuration(&lab);
    let mut store = Store::open(&lab.database("r1"), configuration, "r1").unwrap();
    let mut zero = audit(
        "reply-failure-zero",
        "reply-failure-zero",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::Unavailable,
        Some("reply-failure-zero-wire"),
    );
    zero.authenticated_peer_id = Some("r2".into());
    zero.reply_frame_bytes = 0;
    assert_eq!(
        store.record_exchange_audit(&zero).unwrap_err(),
        DurableError::InvalidAudit(
            "unavailable reply write requires a nonzero partial frame".into()
        )
    );

    let mut full = zero.clone();
    full.audit_event_id = "reply-failure-full".into();
    full.reply_frame_bytes = full.reply_announced_body_bytes.unwrap() + 4;
    assert_eq!(
        store.record_exchange_audit(&full).unwrap_err(),
        DurableError::InvalidAudit(
            "unavailable reply write requires a nonzero partial frame".into()
        )
    );

    for (event_id, phase, outcome) in [
        (
            "reply-partial-observed",
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
        ),
        (
            "reply-partial-refused",
            AuditPhase::InboundRefusalRecorded,
            AuditOutcome::AuthenticatedRefusal,
        ),
        (
            "reply-partial-prepared",
            AuditPhase::InboundReplyPrepared,
            AuditOutcome::Incomplete,
        ),
    ] {
        store
            .record_exchange_audit(&audit(
                event_id,
                "reply-partial",
                phase,
                outcome,
                Some("reply-partial-wire"),
            ))
            .unwrap();
    }
    let mut partial = audit(
        "reply-failure-partial",
        "reply-partial",
        AuditPhase::InboundReplyWriteObserved,
        AuditOutcome::Unavailable,
        Some("reply-partial-wire"),
    );
    partial.authenticated_peer_id = Some("r2".into());
    partial.audit_event_id = "reply-failure-partial".into();
    partial.reply_frame_bytes = 1;
    store.record_exchange_audit(&partial).unwrap();

    let mut diagnostic = audit(
        "diagnostic-partial",
        "diagnostic-partial",
        AuditPhase::InboundDiagnosticReplyWritten,
        AuditOutcome::Malformed,
        None,
    );
    diagnostic.reply_frame_bytes -= 1;
    assert_eq!(
        store.record_exchange_audit(&diagnostic).unwrap_err(),
        DurableError::InvalidAudit(
            "partial diagnostic reply write must record unavailability".into()
        )
    );
}

#[test]
fn authenticated_import_namespaces_local_receipts_and_exposes_unaudited_lab_imports() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "source-r1", "one", false));
    lab.run("r3", &observation(3, "source-r3", "three", false));
    let configuration = store_configuration(&lab);
    let r1 = lab.snapshot("r1");
    let r3 = lab.snapshot("r3");
    lab.import("r2", r1.clone(), "same-wire-operation");
    let mut store = Store::open(&lab.database("r2"), configuration.clone(), "r2").unwrap();
    let reserved = authenticated_import_receipt_id(
        &configuration.topology().unwrap(),
        "r1",
        "same-wire-operation",
    )
    .unwrap();
    assert_eq!(
        store
            .execute_with_receipt(&observation(2, &reserved, "squat", false))
            .unwrap_err(),
        DurableError::Refused(RefusalReason::InvalidRequest)
    );
    assert_eq!(
        store
            .execute_with_receipt(&Request::Import {
                operation_id: reserved,
                snapshot: r1.clone(),
            })
            .unwrap_err(),
        DurableError::Refused(RefusalReason::InvalidRequest)
    );
    let mut local_receipts = Vec::new();
    for (index, snapshot) in [r1, r3].into_iter().enumerate() {
        let attempt_id = format!("authenticated-attempt-{index}");
        let mut observed = audit(
            &format!("authenticated-observed-{index}"),
            &attempt_id,
            AuditPhase::InboundRequestObserved,
            AuditOutcome::Accepted,
            Some("same-wire-operation"),
        );
        observed.authenticated_peer_id = Some(snapshot.replica_id.clone());
        observed.peer_claim = Some(snapshot.replica_id.clone());
        store.record_exchange_audit(&observed).unwrap();
        let mut committed = audit(
            &format!("authenticated-committed-{index}"),
            &attempt_id,
            AuditPhase::InboundImportCommitted,
            AuditOutcome::Accepted,
            Some("same-wire-operation"),
        );
        committed.authenticated_peer_id = Some(snapshot.replica_id.clone());
        committed.peer_claim = Some(snapshot.replica_id.clone());
        let result = store
            .execute_authenticated_import("same-wire-operation", &snapshot, committed)
            .unwrap();
        let receipt = result.executed.receipt.unwrap();
        assert_eq!(receipt.kind, ReceiptKind::AuthenticatedImport);
        assert_eq!(
            receipt.wire_operation_id.as_deref(),
            Some("same-wire-operation")
        );
        assert!(receipt.operation_id.starts_with("network:"));
        local_receipts.push(receipt.operation_id);
    }
    assert_ne!(local_receipts[0], local_receipts[1]);
    assert_ne!(local_receipts[0], "same-wire-operation");
    let inspection = inspect_read_only(&lab.database("r2"), &configuration, "r2").unwrap();
    assert_eq!(
        inspection.unaudited_import_receipt_ids,
        ["same-wire-operation"]
    );
}

#[test]
fn stored_audit_corruption_and_missing_receipt_links_have_exact_error_classes() {
    for case in ["record", "receipt"] {
        let lab = Lab::new();
        let configuration = store_configuration(&lab);
        let path = lab.database("r1");
        let mut store = Store::open(&path, configuration.clone(), "r1").unwrap();
        let prepared = audit(
            "stored-event",
            "stored-attempt",
            AuditPhase::OutboundRequestPrepared,
            AuditOutcome::Incomplete,
            Some("stored-operation"),
        );
        store.record_exchange_audit(&prepared).unwrap();
        drop(store);
        let database = Connection::open(&path).unwrap();
        database
            .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
            .unwrap();
        let mut changed = prepared.clone();
        if case == "record" {
            changed.request_frame_bytes += 1;
        } else {
            changed.local_receipt_operation_id = Some("missing-receipt".into());
            changed.local_receipt_sha256 = Some(format!("{:064x}", 99));
        }
        let record_json = serde_json::to_string(&changed).unwrap();
        let checksum = audit_checksum(&record_json);
        database
            .execute(
                "UPDATE exchange_audit_events SET record_json=?1, sha256=?2,
                 local_receipt_operation_id=?3, local_receipt_sha256=?4
                 WHERE audit_event_id='stored-event'",
                params![
                    record_json,
                    checksum,
                    changed.local_receipt_operation_id,
                    changed.local_receipt_sha256,
                ],
            )
            .unwrap();
        database
            .execute_batch(
                "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
            )
            .unwrap();
        let error = inspect_read_only(&path, &configuration, "r1").unwrap_err();
        if case == "record" {
            assert_eq!(
                error,
                DurableError::Corrupt("stored audit record or hash mismatch".into())
            );
        } else {
            assert_eq!(
                error,
                DurableError::Corrupt(
                    "stored audit receipt link is invalid: invalid_audit: local receipt reference is missing or has a mismatched digest"
                        .into()
                )
            );
        }
    }
}

#[test]
fn wal_v2_refusal_and_symlink_inspection_preserve_source_bytes() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};

    let lab = Lab::new();
    let path = lab.database("r1");
    let marker = lab.directory.path().join("legacy-v2-live");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash_worker", "--nocapture"])
        .env("MANAGER_LAB_CRASH_DB", &path)
        .env("MANAGER_LAB_CRASH_CONFIG", &lab.config_path)
        .env("MANAGER_LAB_CRASH_MARKER", &marker)
        .env("MANAGER_LAB_CRASH_PHASE", "legacy_v2_wal")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        assert!(child.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let shm = PathBuf::from(format!("{}-shm", path.display()));
    assert!(fs::metadata(&wal).unwrap().len() > 0);
    assert!(fs::metadata(&shm).unwrap().len() > 0);
    let before = directory_state(lab.directory.path());
    assert_eq!(
        Store::open(&path, store_configuration(&lab), "r1")
            .err()
            .unwrap(),
        DurableError::Refused(RefusalReason::UnsupportedSchema)
    );
    assert_eq!(before, directory_state(lab.directory.path()));

    let valid = lab.database("r2");
    Store::open(&valid, store_configuration(&lab), "r2").unwrap();
    let link = lab.directory.path().join("linked.sqlite");
    symlink(&valid, &link).unwrap();
    assert_eq!(
        inspect_read_only(&link, &store_configuration(&lab), "r2").unwrap_err(),
        DurableError::Refused(RefusalReason::UnsafeStore)
    );
    assert_eq!(
        Store::open(&link, store_configuration(&lab), "r2")
            .err()
            .unwrap(),
        DurableError::Refused(RefusalReason::UnsafeStore)
    );

    let sidecar_link = PathBuf::from(format!("{}-wal", valid.display()));
    symlink(&path, &sidecar_link).unwrap();
    assert_eq!(
        inspect_read_only(&valid, &store_configuration(&lab), "r2").unwrap_err(),
        DurableError::Refused(RefusalReason::UnsafeStore)
    );
    fs::remove_file(&sidecar_link).unwrap();

    assert!(Command::new("mkfifo")
        .arg(&sidecar_link)
        .status()
        .unwrap()
        .success());
    let started = Instant::now();
    assert_eq!(
        inspect_read_only(&valid, &store_configuration(&lab), "r2").unwrap_err(),
        DurableError::Refused(RefusalReason::UnsafeStore)
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    fs::remove_file(&sidecar_link).unwrap();

    let non_utf8 = lab.directory.path().join(std::ffi::OsString::from_vec(
        b"non-utf8-\xff.sqlite".to_vec(),
    ));
    let mut non_utf8_store = Store::open(&non_utf8, store_configuration(&lab), "r3").unwrap();
    non_utf8_store
        .execute(&observation(3, "non-utf8-write", "visible", false))
        .unwrap();
    assert_eq!(
        inspect_read_only(&non_utf8, &store_configuration(&lab), "r3")
            .unwrap()
            .history_count,
        1
    );
}

#[test]
fn killed_child_v3_wal_identity_mismatch_preserves_every_store_file() {
    let lab = Lab::new();
    let path = lab.database("r1");
    let marker = lab.directory.path().join("v3-live");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "crash_worker", "--nocapture"])
        .env("MANAGER_LAB_CRASH_DB", &path)
        .env("MANAGER_LAB_CRASH_CONFIG", &lab.config_path)
        .env("MANAGER_LAB_CRASH_MARKER", &marker)
        .env("MANAGER_LAB_CRASH_PHASE", "live_v3_wal")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        assert!(child.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    assert!(fs::metadata(&wal).unwrap().len() > 0);
    let before = directory_state(lab.directory.path());
    let configuration = store_configuration(&lab);
    assert_eq!(
        inspect_read_only(&path, &configuration, "r2").unwrap_err(),
        DurableError::Refused(RefusalReason::IdentityMismatch)
    );
    assert_eq!(before, directory_state(lab.directory.path()));
    assert_eq!(
        Store::open(&path, configuration, "r2").err().unwrap(),
        DurableError::Refused(RefusalReason::IdentityMismatch)
    );
    assert_eq!(before, directory_state(lab.directory.path()));
}

#[test]
fn external_inspector_is_read_only_and_unexpected_schema_objects_are_refused() {
    let lab = Lab::new();
    lab.run("r1", &observation(1, "inspected", "value", false));
    let before = directory_state(lab.directory.path());
    let output = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-ha-lab"))
        .args([
            "--inspect-store".as_ref(),
            lab.database("r1").as_os_str(),
            lab.config_path.as_os_str(),
            "r1".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let inspection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspection["history_count"], 1);
    assert_eq!(before, directory_state(lab.directory.path()));

    let connection = Connection::open(lab.database("r1")).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER sqlitex_hidden BEFORE INSERT ON facts BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
    assert_eq!(
        inspect_read_only(&lab.database("r1"), &store_configuration(&lab), "r1").unwrap_err(),
        DurableError::Corrupt("manager store schema shape mismatch".into())
    );
}

fn audit_checksum(record_json: &str) -> String {
    let frame = serde_json::to_vec(&("podmesh-manager-ha-exchange-audit/1", record_json)).unwrap();
    format!("{:x}", Sha256::digest(frame))
}

fn store_configuration(lab: &Lab) -> Configuration {
    serde_json::from_slice(&fs::read(&lab.config_path).unwrap()).unwrap()
}

fn directory_state(path: &Path) -> Vec<(String, u64, String, u64)> {
    let mut state: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                metadata.len(),
                format!("{:x}", Sha256::digest(fs::read(entry.path()).unwrap())),
                metadata
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
                    .try_into()
                    .unwrap_or(u64::MAX),
            )
        })
        .collect();
    state.sort_by(|left, right| left.0.cmp(&right.0));
    state
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
    let phase = std::env::var("MANAGER_LAB_CRASH_PHASE").unwrap();
    let replica = std::env::var("MANAGER_LAB_CRASH_REPLICA").unwrap_or_else(|_| "r1".into());
    if phase == "legacy_v2_wal" {
        let connection = Connection::open(database).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE legacy (value TEXT);
                 PRAGMA user_version=2;
                 INSERT INTO legacy VALUES ('uncheckpointed');",
            )
            .unwrap();
        fs::write(marker, b"legacy WAL remains live").unwrap();
        loop {
            thread::park();
        }
    } else if phase == "live_v3_wal" {
        let mut store = Store::open(Path::new(&database), configuration, &replica).unwrap();
        let scope_number = replica
            .strip_prefix('r')
            .and_then(|number| number.parse::<u32>().ok())
            .unwrap();
        store
            .execute(&observation(
                scope_number,
                "v3-live-write",
                "uncheckpointed",
                false,
            ))
            .unwrap();
        fs::write(marker, b"v3 WAL remains live").unwrap();
        loop {
            thread::park();
        }
    } else if phase == "after_commit" {
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
