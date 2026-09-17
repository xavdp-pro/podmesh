//! The store integrity model: a complete verification when a process first
//! opens a store and at each periodic pass, verification of appended rows in
//! every transaction, and a store that fails closed for the rest of the process
//! once any verification fails.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use podmesh_manager_ha_lab::{
    durable::{
        AuditDirection, AuditErrorCategory, AuditOutcome, AuditPhase, Configuration, DurableError,
        ExchangeAuditEvent, RefusalReason, Request, Response, Store,
    },
    ReplicaConfig, ScopeGrant,
};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Lab {
    directory: TempDir,
}

impl Lab {
    fn new() -> Self {
        let lab = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        fs::write(
            lab.configuration_path(),
            serde_json::to_vec(&configuration()).unwrap(),
        )
        .unwrap();
        lab
    }

    fn database(&self) -> PathBuf {
        self.directory.path().join("r1.sqlite")
    }

    fn configuration_path(&self) -> PathBuf {
        self.directory.path().join("configuration.json")
    }

    fn open(&self) -> Store {
        Store::open(&self.database(), configuration(), "r1").unwrap()
    }

    /// One request through the one-request-per-process executable: a process
    /// that opens this store for the first time.
    fn request_in_new_process(&self, request: &Request) -> (bool, serde_json::Value) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-ha-lab"))
            .args([
                self.database().as_os_str(),
                self.configuration_path().as_os_str(),
                "r1".as_ref(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).unwrap())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        (
            output.status.success(),
            serde_json::from_slice(&output.stdout).unwrap(),
        )
    }

    fn audit_rows(&self) -> usize {
        Connection::open(self.database())
            .unwrap()
            .query_row("SELECT count(*) FROM exchange_audit_events", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// Edits one stored audit row with its no-update trigger dropped, in one
    /// SQLite transaction, as an actor bypassing the Store would.
    fn edit_audit_row(&self, audit_event_id: &str) {
        let mut connection = Connection::open(self.database()).unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
            .unwrap();
        assert_eq!(
            transaction
                .execute(
                    "UPDATE exchange_audit_events SET request_frame_bytes=999 WHERE audit_event_id=?1",
                    [audit_event_id],
                )
                .unwrap(),
            1
        );
        transaction
            .execute_batch(
                "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
            )
            .unwrap();
        transaction.commit().unwrap();
    }
}

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

fn observation(operation_id: &str, value: &str) -> Request {
    Request::Observe {
        operation_id: operation_id.into(),
        scope: "scope1".into(),
        subject: "integrity".into(),
        exclusive_resource: None,
        active_claim: false,
        value: value.into(),
    }
}

/// An outbound attempt's prepared phase: zero transferred request bytes and the
/// digest and size of the request it intends to send.
fn prepared(label: &str) -> ExchangeAuditEvent {
    let hash = format!("{:x}", Sha256::digest(label.as_bytes()));
    ExchangeAuditEvent {
        audit_event_id: format!("audit-{label}"),
        attempt_id: format!("attempt:{hash}"),
        wire_nonce: format!("nonce-{}", &hash[..16]),
        direction: AuditDirection::Outbound,
        phase: AuditPhase::OutboundRequestPrepared,
        authenticated_peer_id: Some("r2".into()),
        peer_claim: Some("r2".into()),
        operation_id: Some(format!("operation-{label}")),
        request_frame_bytes: 0,
        request_announced_body_bytes: Some(128),
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

fn edited_prepared_row_error() -> DurableError {
    DurableError::Corrupt(
        "stored audit is invalid: invalid_audit: outbound request preparation records intent with zero transferred bytes"
            .into(),
    )
}

fn assert_failed_closed(lab: &Lab, store: &mut Store, expected: &DurableError) {
    let rows = lab.audit_rows();
    assert_eq!(&store.execute(&Request::Export {}).unwrap_err(), expected);
    assert_eq!(&store.execute(&Request::Inspect {}).unwrap_err(), expected);
    assert_eq!(
        &store
            .execute_with_receipt(&observation("after-failure", "refused"))
            .unwrap_err(),
        expected
    );
    assert_eq!(
        &store
            .record_exchange_audit(&prepared("after-failure"))
            .unwrap_err(),
        expected
    );
    assert_eq!(&store.verify_full().unwrap_err(), expected);
    assert_eq!(
        &Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        expected
    );
    assert_eq!(store.integrity().unwrap().failure.as_ref(), Some(expected));
    assert_eq!(lab.audit_rows(), rows);
}

#[test]
fn edited_old_audit_row_is_detected_at_first_open_and_by_the_periodic_pass_then_fails_closed() {
    let lab = Lab::new();
    let mut store = lab.open();
    for label in ["first", "second", "third"] {
        store.record_exchange_audit(&prepared(label)).unwrap();
    }
    store
        .execute_with_receipt(&observation("before-edit", "value"))
        .unwrap();
    lab.edit_audit_row("audit-first");

    // The edited row is neither appended nor read by these operations, and it
    // is not the last verified row: detection waits for a complete verification.
    store.execute(&Request::Export {}).unwrap();
    store.record_exchange_audit(&prepared("fourth")).unwrap();

    // A process that opens the store for the first time verifies every row.
    let (success, reply) = lab.request_in_new_process(&Request::Export {});
    assert!(!success);
    assert_eq!(reply["error"], edited_prepared_row_error().to_string());

    // The periodic pass of this process detects the edit and fails the store
    // closed for every later operation, including a new open.
    assert_eq!(
        store.verify_full().unwrap_err(),
        edited_prepared_row_error()
    );
    assert_failed_closed(&lab, &mut store, &edited_prepared_row_error());
}

#[test]
fn complete_verification_runs_once_per_process_and_follows_the_interval() {
    let lab = Lab::new();
    let mut store = lab.open();
    assert_eq!(store.integrity().unwrap().full_verifications, 1);
    store.record_exchange_audit(&prepared("once")).unwrap();
    for _ in 0..5 {
        let mut again = lab.open();
        again.execute(&Request::Export {}).unwrap();
        assert_eq!(again.integrity().unwrap().full_verifications, 1);
    }
    assert!(!store.verify_full_if_due(Duration::from_secs(3600)).unwrap());
    assert_eq!(store.integrity().unwrap().full_verifications, 1);
    assert!(store.verify_full_if_due(Duration::ZERO).unwrap());
    let integrity = store.integrity().unwrap();
    assert_eq!(integrity.full_verifications, 2);
    assert!(integrity.failure.is_none());
    assert!(integrity.last_full_verification_age.unwrap() < Duration::from_secs(60));
}

#[test]
fn rows_appended_by_another_writer_are_verified_by_the_next_transaction() {
    let lab = Lab::new();
    let mut store = lab.open();
    store.record_exchange_audit(&prepared("verified")).unwrap();

    // A legitimate row appended by another process is verified and accepted.
    let (success, _) = lab.request_in_new_process(&observation("other-process", "value"));
    assert!(success);
    store.execute(&Request::Export {}).unwrap();

    // A malformed row appended without the Store is refused by the next
    // transaction, even one that reads no audit row, without a complete pass.
    Connection::open(lab.database())
        .unwrap()
        .execute(
            "INSERT INTO exchange_audit_events VALUES ('forged', 'attempt:forged', 'nonce', 'outbound', 'outbound_request_prepared', NULL, NULL, NULL, 0, NULL, NULL, 0, NULL, NULL, 'incomplete', NULL, NULL, NULL, NULL, NULL, NULL, 0, '{}', ?1)",
            params!["0".repeat(64)],
        )
        .unwrap();
    let error = store.execute(&Request::Export {}).unwrap_err();
    assert!(matches!(error, DurableError::Corrupt(_)), "{error:?}");
    assert_eq!(store.integrity().unwrap().full_verifications, 1);
    assert_failed_closed(&lab, &mut store, &error);
}

#[test]
fn edited_last_verified_row_is_detected_by_the_next_transaction() {
    let lab = Lab::new();
    let mut store = lab.open();
    store.record_exchange_audit(&prepared("older")).unwrap();
    store.record_exchange_audit(&prepared("latest")).unwrap();
    // This transaction verifies both rows; the latest becomes the last verified.
    store.execute(&Request::Export {}).unwrap();
    let mut connection = Connection::open(lab.database()).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    transaction
        .execute(
            "UPDATE exchange_audit_events SET sha256=?1 WHERE audit_event_id='audit-latest'",
            params!["f".repeat(64)],
        )
        .unwrap();
    transaction
        .execute_batch(
            "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;",
        )
        .unwrap();
    transaction.commit().unwrap();
    let expected = DurableError::Corrupt("stored audit record or hash mismatch".into());
    assert_eq!(store.execute(&Request::Export {}).unwrap_err(), expected);
    assert_failed_closed(&lab, &mut store, &expected);
}

#[test]
fn replayed_receipt_and_audit_rows_are_verified_before_replay() {
    let lab = Lab::new();
    let mut store = lab.open();
    let request = observation("replayed", "original");
    let original = store.execute_with_receipt(&request).unwrap();
    assert_eq!(store.execute_with_receipt(&request).unwrap(), {
        let mut replay = original.clone();
        replay.replayed = true;
        replay
    });
    store
        .record_exchange_audit(&prepared("replayed-audit"))
        .unwrap();
    store.record_exchange_audit(&prepared("later")).unwrap();
    store.execute(&Request::Export {}).unwrap();

    // An old receipt edited in place is refused when a retry would replay it.
    let Response::Observed { mut fact } = original.response else {
        panic!("observation expected");
    };
    fact.value = "forged".into();
    let mut connection = Connection::open(lab.database()).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute_batch("DROP TRIGGER receipts_no_update")
        .unwrap();
    transaction
        .execute(
            "UPDATE receipts SET response_json=?1 WHERE operation_id='replayed'",
            [serde_json::to_string(&Response::Observed { fact }).unwrap()],
        )
        .unwrap();
    transaction
        .execute_batch(
            "CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;",
        )
        .unwrap();
    transaction.commit().unwrap();
    let expected = DurableError::Corrupt("stored receipt hash mismatch".into());
    assert_eq!(store.execute_with_receipt(&request).unwrap_err(), expected);
    assert_failed_closed(&lab, &mut store, &expected);

    // An old audit row edited in place is refused when an identical event is
    // replayed, in a process that has not yet verified it completely.
    let lab = Lab::new();
    let mut store = lab.open();
    let event = prepared("replayed-audit");
    store.record_exchange_audit(&event).unwrap();
    store.record_exchange_audit(&prepared("later")).unwrap();
    store.execute(&Request::Export {}).unwrap();
    lab.edit_audit_row("audit-replayed-audit");
    assert_eq!(
        store.record_exchange_audit(&event).unwrap_err(),
        edited_prepared_row_error()
    );
    assert_failed_closed(&lab, &mut store, &edited_prepared_row_error());
}

#[test]
fn candidate_audit_is_checked_against_the_rows_of_its_own_attempt() {
    let lab = Lab::new();
    let mut store = lab.open();
    let first = prepared("attempt-one");
    store.record_exchange_audit(&first).unwrap();
    // Unrelated attempts before and after do not change the attempt's rules.
    store
        .record_exchange_audit(&prepared("attempt-two"))
        .unwrap();
    let mut duplicate_phase = first.clone();
    duplicate_phase.audit_event_id = "audit-duplicate-phase".into();
    assert_eq!(
        store.record_exchange_audit(&duplicate_phase).unwrap_err(),
        DurableError::InvalidAudit("exchange attempt already contains this phase".into())
    );
    let mut other_direction = first.clone();
    other_direction.audit_event_id = "audit-other-direction".into();
    other_direction.direction = AuditDirection::Inbound;
    other_direction.phase = AuditPhase::InboundRequestObserved;
    other_direction.outcome = AuditOutcome::Accepted;
    other_direction.request_frame_bytes = 132;
    assert_eq!(
        store.record_exchange_audit(&other_direction).unwrap_err(),
        DurableError::InvalidAudit("one attempt ID cannot span both audit directions".into())
    );
    let mut orphan = prepared("attempt-three");
    orphan.audit_event_id = "audit-orphan-completion".into();
    orphan.phase = AuditPhase::OutboundExchangeCompleted;
    orphan.outcome = AuditOutcome::Unavailable;
    orphan.authenticated_peer_id = None;
    orphan.error_category = Some(AuditErrorCategory::Unavailable);
    orphan.reason_code = Some(RefusalReason::TransportUnavailable);
    assert_eq!(
        store.record_exchange_audit(&orphan).unwrap_err(),
        DurableError::InvalidAudit("outbound exchange lacks its prepared predecessor".into())
    );
    assert!(store.integrity().unwrap().failure.is_none());
}

#[test]
fn a_store_that_fails_verification_at_first_open_stays_closed() {
    let lab = Lab::new();
    {
        let mut store = lab.open();
        store.record_exchange_audit(&prepared("stored")).unwrap();
        store.record_exchange_audit(&prepared("next")).unwrap();
    }
    lab.edit_audit_row("audit-stored");
    // This process opened the store before the edit; a new process has not.
    let (success, reply) = lab.request_in_new_process(&Request::Inspect {});
    assert!(!success);
    assert_eq!(reply["error"], edited_prepared_row_error().to_string());
    let copy = copy_store(&lab);
    let error = Store::open(&copy, configuration(), "r1").err().unwrap();
    assert_eq!(error, edited_prepared_row_error());
    assert_eq!(
        Store::open(&copy, configuration(), "r1").err().unwrap(),
        edited_prepared_row_error()
    );
}

/// A byte copy of a closed store is a different file, first opened by this
/// process.
fn copy_store(lab: &Lab) -> PathBuf {
    let copy = lab.directory.path().join("copy.sqlite");
    Connection::open(lab.database())
        .unwrap()
        .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
        .unwrap();
    assert!(Path::new(&copy).exists());
    copy
}
