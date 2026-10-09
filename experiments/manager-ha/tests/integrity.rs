//! The store integrity model: a complete verification when a process first
//! opens a store and at each periodic pass, verification of appended rows in
//! every transaction, and a store that fails closed for the rest of the process
//! once any verification fails.
use std::{
    env, fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use podmesh_manager_ha_lab::{
    durable::{
        inspect_facts_read_only, inspect_read_only, AuditDirection, AuditErrorCategory,
        AuditOutcome, AuditPhase, Configuration, DurableError, ExchangeAuditEvent, RefusalReason,
        Request, Response, Store,
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

    /// Changes one stored row of an append-only table with the table's no-update
    /// trigger dropped, in one SQLite transaction, as an actor bypassing the Store
    /// would.
    fn edit_row(&self, table: &str, assignment: &str, condition: &str) {
        let (trigger, message) = match table {
            "facts" => ("facts_no_update", "immutable fact"),
            "receipts" => ("receipts_no_update", "immutable receipt"),
            _ => (
                "exchange_audit_events_no_update",
                "immutable exchange audit event",
            ),
        };
        let mut connection = Connection::open(self.database()).unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
        assert_eq!(
            transaction
                .execute(
                    &format!("UPDATE {table} SET {assignment} WHERE {condition}"),
                    [],
                )
                .unwrap(),
            1
        );
        transaction
            .execute_batch(&format!(
                "CREATE TRIGGER {trigger} BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT, '{message}'); END;"
            ))
            .unwrap();
        transaction.commit().unwrap();
    }
}

/// Another replica's store in the same laboratory, with one fact of its own
/// origin and the receipt of that observation. Both rows verify in the first
/// replica's store too: the fact is one an import would carry, and the receipt
/// binds the same logical manager.
fn other_replica_store(lab: &Lab) -> PathBuf {
    let path = lab.directory.path().join("r2.sqlite");
    Store::open(&path, configuration(), "r2")
        .unwrap()
        .execute(&Request::Observe {
            operation_id: "other-observation".into(),
            scope: "scope2".into(),
            subject: "other".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "other value".into(),
        })
        .unwrap();
    path
}

/// Copies one row of another replica's store into this one, at a rowid three
/// beyond the last: a row that any writer could add, after a gap.
fn insert_row_after_a_gap(lab: &Lab, table: &str) {
    let other = other_replica_store(lab);
    let columns = match table {
        "facts" => "event_id, fact_json, sha256",
        _ => RECEIPT_COLUMNS,
    };
    let source = Connection::open(&other).unwrap();
    let mut statement = source
        .prepare(&format!(
            "SELECT {columns} FROM {table} ORDER BY rowid LIMIT 1"
        ))
        .unwrap();
    let values: Vec<rusqlite::types::Value> = statement
        .query_row([], |row| {
            (0..columns.split(',').count())
                .map(|index| row.get::<_, rusqlite::types::Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap();
    drop(statement);
    drop(source);
    let placeholders: Vec<_> = (1..=values.len()).map(|n| format!("?{n}")).collect();
    let destination = Connection::open(lab.database()).unwrap();
    destination
        .execute(
            &format!(
                "INSERT INTO {table} (rowid, {columns}) VALUES (
                     (SELECT max(rowid) + 3 FROM {table}), {})",
                placeholders.join(", ")
            ),
            rusqlite::params_from_iter(values),
        )
        .unwrap();
}

const RECEIPT_COLUMNS: &str =
    "operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256";

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

/// An accepted inbound request observation of one attempt, with its request bytes.
fn inbound_observed(label: &str, operation: &str) -> ExchangeAuditEvent {
    let hash = format!("{:x}", Sha256::digest(label.as_bytes()));
    ExchangeAuditEvent {
        audit_event_id: format!("audit-{label}-observed"),
        attempt_id: format!("attempt:{hash}"),
        wire_nonce: format!("nonce-{}", &hash[..16]),
        direction: AuditDirection::Inbound,
        phase: AuditPhase::InboundRequestObserved,
        authenticated_peer_id: Some("r2".into()),
        peer_claim: Some("r2".into()),
        operation_id: Some(operation.into()),
        request_frame_bytes: 132,
        request_announced_body_bytes: Some(128),
        request_sha256: Some(format!("{:064x}", 1)),
        reply_frame_bytes: 0,
        reply_announced_body_bytes: None,
        reply_sha256: None,
        outcome: AuditOutcome::Accepted,
        error_category: None,
        reason_code: None,
        local_receipt_operation_id: None,
        local_receipt_sha256: None,
        remote_receipt_operation_id: None,
        remote_receipt_sha256: None,
        replayed: false,
    }
}

/// The accepted import decision of that attempt, as the atomic API takes it.
fn inbound_import(label: &str, operation: &str) -> ExchangeAuditEvent {
    let mut event = inbound_observed(label, operation);
    event.audit_event_id = format!("audit-{label}-imported");
    event.phase = AuditPhase::InboundImportCommitted;
    event.request_frame_bytes = 0;
    event.request_announced_body_bytes = None;
    event.request_sha256 = None;
    event
}

fn edited_prepared_row_error() -> DurableError {
    DurableError::Corrupt(
        "stored audit is invalid: invalid_audit: outbound request preparation records intent with zero transferred bytes"
            .into(),
    )
}

fn schema_mismatch() -> DurableError {
    DurableError::Corrupt("manager store schema shape mismatch".into())
}

const AUDIT_NO_UPDATE_TRIGGER: &str = "CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;";

fn rowid_gap(table: &str) -> DurableError {
    DurableError::Corrupt(format!("stored {table} rowids are not contiguous from 1"))
}

const AUDIT_COLUMNS: &str = "audit_event_id, attempt_id, wire_nonce, direction, phase,
    authenticated_peer_id, peer_claim, operation_id, request_frame_bytes,
    request_announced_body_bytes, request_sha256, reply_frame_bytes,
    reply_announced_body_bytes, reply_sha256, outcome, error_category, reason_code,
    local_receipt_operation_id, local_receipt_sha256,
    remote_receipt_operation_id, remote_receipt_sha256, replayed, record_json, sha256";

fn enum_text(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

/// Inserts one well-formed audit row exactly as the Store writes it, but at an
/// explicit rowid and without the Store. The schema allows it: no trigger is
/// dropped.
fn insert_audit_row_at(lab: &Lab, rowid: i64, event: &ExchangeAuditEvent) {
    let record_json = serde_json::to_string(event).unwrap();
    let checksum = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&("podmesh-manager-ha-exchange-audit/1", &record_json)).unwrap()
        )
    );
    let size = |value: Option<u64>| value.map(|value| i64::try_from(value).unwrap());
    Connection::open(lab.database())
        .unwrap()
        .execute(
            &format!(
                "INSERT INTO exchange_audit_events (rowid, {AUDIT_COLUMNS}) VALUES (
                    ?25, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)"
            ),
            params![
                event.audit_event_id,
                event.attempt_id,
                event.wire_nonce,
                enum_text(&event.direction),
                enum_text(&event.phase),
                event.authenticated_peer_id,
                event.peer_claim,
                event.operation_id,
                i64::try_from(event.request_frame_bytes).unwrap(),
                size(event.request_announced_body_bytes),
                event.request_sha256,
                i64::try_from(event.reply_frame_bytes).unwrap(),
                size(event.reply_announced_body_bytes),
                event.reply_sha256,
                enum_text(&event.outcome),
                event.error_category.map(|value| enum_text(&value)),
                event.reason_code.map(|value| enum_text(&value)),
                event.local_receipt_operation_id,
                event.local_receipt_sha256,
                event.remote_receipt_operation_id,
                event.remote_receipt_sha256,
                i64::from(event.replayed),
                record_json,
                checksum,
                rowid,
            ],
        )
        .unwrap();
}

/// Inserts the malformed audit row of
/// `rows_appended_by_another_writer_are_verified_by_the_next_transaction` at an
/// explicit rowid, with a plain SQL insert.
fn insert_malformed_audit_row_at(lab: &Lab, rowid: i64, audit_event_id: &str) {
    Connection::open(lab.database())
        .unwrap()
        .execute(
            &format!(
                "INSERT INTO exchange_audit_events (rowid, {AUDIT_COLUMNS}) VALUES (?1, ?2, 'attempt:forged', 'nonce', 'outbound', 'outbound_request_prepared', NULL, NULL, NULL, 0, NULL, NULL, 0, NULL, NULL, 'incomplete', NULL, NULL, NULL, NULL, NULL, NULL, 0, '{{}}', ?3)"
            ),
            params![rowid, audit_event_id, "0".repeat(64)],
        )
        .unwrap();
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

/// Review of lot V2-R, probe P1: a malformed audit row added by a plain SQL
/// insert, no trigger dropped, at a rowid before the first row. Before the rowid
/// rule, exports, audit records, observations and a new open in the same process
/// all succeeded, and only a complete verification refused the store.
#[test]
fn a_row_added_before_the_first_row_is_refused_by_the_next_transaction() {
    let lab = Lab::new();
    let mut store = lab.open();
    store.record_exchange_audit(&prepared("verified")).unwrap();
    store.execute(&Request::Export {}).unwrap();

    insert_malformed_audit_row_at(&lab, -1, "forged-before-first");
    let expected = rowid_gap("exchange_audit_events");
    assert_eq!(store.execute(&Request::Export {}).unwrap_err(), expected);
    assert_eq!(store.integrity().unwrap().full_verifications, 1);
    assert_failed_closed(&lab, &mut store, &expected);
    let (success, reply) = lab.request_in_new_process(&Request::Export {});
    assert!(!success);
    assert_eq!(reply["error"], expected.to_string());
}

/// Probe P2: a corrupt fact before the first fact is refused by a transaction that
/// only records an audit row and loads no fact.
#[test]
fn a_fact_added_before_the_first_fact_is_refused_by_a_transaction_that_loads_no_fact() {
    let lab = Lab::new();
    let mut store = lab.open();
    store
        .execute_with_receipt(&observation("first", "value"))
        .unwrap();
    store.execute(&Request::Export {}).unwrap();

    Connection::open(lab.database())
        .unwrap()
        .execute(
            "INSERT INTO facts(rowid, event_id, fact_json, sha256) VALUES (-7, 'forged-fact', '{}', ?1)",
            [format!("{:064x}", 0)],
        )
        .unwrap();
    let expected = rowid_gap("facts");
    assert_eq!(
        store
            .record_exchange_audit(&prepared("audit-only"))
            .unwrap_err(),
        expected
    );
    assert_failed_closed(&lab, &mut store, &expected);
}

/// Probe P1b: one well-formed row after a gap moved the last verified row past
/// the gap, and malformed rows then added inside it passed every transaction. The
/// rows after the last verified row must continue the table's rowids, whoever
/// wrote them.
#[test]
fn a_row_added_after_a_gap_is_refused_by_the_next_transaction() {
    let lab = Lab::new();
    let mut store = lab.open();
    store.record_exchange_audit(&prepared("verified")).unwrap();
    store.execute(&Request::Export {}).unwrap();

    // A well-formed row that continues the rowids is verified and accepted,
    // although no Store wrote it.
    insert_audit_row_at(&lab, 2, &prepared("next"));
    store.execute(&Request::Export {}).unwrap();

    insert_audit_row_at(&lab, 1_000_000, &prepared("after-gap"));
    let expected = rowid_gap("exchange_audit_events");
    assert_eq!(
        store
            .record_exchange_audit(&prepared("refused"))
            .unwrap_err(),
        expected
    );
    assert_failed_closed(&lab, &mut store, &expected);
    // The rows the gap would have hidden are refused with it.
    insert_malformed_audit_row_at(&lab, 500, "forged-in-gap");
    assert_eq!(store.execute(&Request::Export {}).unwrap_err(), expected);
    let (success, reply) = lab.request_in_new_process(&Request::Export {});
    assert!(!success);
    assert_eq!(reply["error"], expected.to_string());
}

/// Probe P7: `REPLACE` deletes the row it conflicts with and inserts the new one.
/// SQLite fires no delete trigger for that implicit delete while
/// `recursive_triggers` is off, its default, and the update trigger does not fire
/// on an insert: an old row changes with every trigger in place. When the row
/// keeps its rowid, only a complete verification reads it again.
#[test]
fn an_old_row_replaced_in_place_is_detected_by_the_complete_verification() {
    let lab = Lab::new();
    let mut store = lab.open();
    store.record_exchange_audit(&prepared("old")).unwrap();
    store.record_exchange_audit(&prepared("newer")).unwrap();
    store.execute(&Request::Export {}).unwrap();

    let connection = Connection::open(lab.database()).unwrap();
    assert!(connection
        .execute(
            "UPDATE exchange_audit_events SET request_frame_bytes=999 WHERE audit_event_id='audit-old'",
            [],
        )
        .is_err());
    assert!(connection
        .execute(
            "DELETE FROM exchange_audit_events WHERE audit_event_id='audit-old'",
            [],
        )
        .is_err());
    let replaced = AUDIT_COLUMNS.replace("request_frame_bytes,", "999,");
    assert_eq!(
        connection
            .execute(
                &format!(
                    "REPLACE INTO exchange_audit_events (rowid, {AUDIT_COLUMNS})
                     SELECT rowid, {replaced} FROM exchange_audit_events WHERE audit_event_id='audit-old'"
                ),
                [],
            )
            .unwrap(),
        1
    );
    let (rowid, bytes): (i64, i64) = connection
        .query_row(
            "SELECT rowid, request_frame_bytes FROM exchange_audit_events WHERE audit_event_id='audit-old'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((rowid, bytes), (1, 999));

    // Neither the last verified row nor a change of the rowids: the next
    // transactions do not read it.
    store.execute(&Request::Export {}).unwrap();
    store
        .record_exchange_audit(&prepared("after-replace"))
        .unwrap();
    assert!(store.integrity().unwrap().failure.is_none());
    assert_eq!(
        store.verify_full().unwrap_err(),
        edited_prepared_row_error()
    );
    assert_failed_closed(&lab, &mut store, &edited_prepared_row_error());
}

fn replace_through_primary_key(lab: &Lab, audit_event_id: &str) {
    Connection::open(lab.database())
        .unwrap()
        .execute(
            &format!(
                "REPLACE INTO exchange_audit_events ({AUDIT_COLUMNS})
                 SELECT {AUDIT_COLUMNS} FROM exchange_audit_events WHERE audit_event_id=?1"
            ),
            [audit_event_id],
        )
        .unwrap();
}

/// A `REPLACE` through the primary key alone deletes the old row and appends its
/// replacement with the next rowid. The replacement is verified by the next
/// transaction like any appended row; the gap left behind is found by the next
/// complete verification, or by the next transaction when the replaced row was
/// the first.
#[test]
fn a_row_replaced_through_its_primary_key_leaves_a_gap_that_is_refused() {
    let lab = Lab::new();
    let mut store = lab.open();
    for label in ["first", "middle", "last"] {
        store.record_exchange_audit(&prepared(label)).unwrap();
    }
    store.execute(&Request::Export {}).unwrap();

    // The same bytes under a new rowid: the appended row verifies, the gap at
    // rowid 2 waits for the complete verification.
    replace_through_primary_key(&lab, "audit-middle");
    store.execute(&Request::Export {}).unwrap();
    assert!(store.integrity().unwrap().failure.is_none());
    let expected = rowid_gap("exchange_audit_events");
    assert_eq!(store.verify_full().unwrap_err(), expected);
    assert_failed_closed(&lab, &mut store, &expected);

    let lab = Lab::new();
    let mut store = lab.open();
    for label in ["first", "middle", "last"] {
        store.record_exchange_audit(&prepared(label)).unwrap();
    }
    store.execute(&Request::Export {}).unwrap();
    replace_through_primary_key(&lab, "audit-first");
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

/// Review of lot V2-R, probe P3: bytes that SQLite cannot return as their column
/// (a BLOB where the Store writes text) in an old fact were met by the fact load
/// of an export, which returned `storage` and closed nothing while audit records
/// kept being written; the same bytes met by the verification of an appended row
/// closed the store. A stored row read at use now closes it too.
#[test]
fn an_unreadable_fact_closes_the_store_whether_loaded_at_use_or_appended() {
    let lab = Lab::new();
    let mut store = lab.open();
    for index in 0..2 {
        store
            .execute_with_receipt(&observation(&format!("fact-{index}"), "value"))
            .unwrap();
    }
    store.execute(&Request::Export {}).unwrap();
    // Not the last verified fact: only a fact load reads it.
    lab.edit_row("facts", "fact_json=X'00'", "rowid=1");
    let error = store.execute(&Request::Export {}).unwrap_err();
    assert!(matches!(error, DurableError::Storage(_)), "{error:?}");
    assert_failed_closed(&lab, &mut store, &error);

    let lab = Lab::new();
    let mut store = lab.open();
    store.execute(&Request::Export {}).unwrap();
    Connection::open(lab.database())
        .unwrap()
        .execute(
            "INSERT INTO facts(event_id, fact_json, sha256) VALUES ('unreadable', X'00', 'x')",
            [],
        )
        .unwrap();
    let appended = store
        .record_exchange_audit(&prepared("after-append"))
        .unwrap_err();
    assert!(matches!(appended, DurableError::Storage(_)), "{appended:?}");
    assert_failed_closed(&lab, &mut store, &appended);
}

/// The stored receipt of a retried operation is read at use before its response
/// is replayed.
#[test]
fn an_unreadable_receipt_met_by_a_replay_closes_the_store() {
    let lab = Lab::new();
    let mut store = lab.open();
    let request = observation("replayed", "original");
    store.execute_with_receipt(&request).unwrap();
    store
        .execute_with_receipt(&observation("later", "value"))
        .unwrap();
    store.execute(&Request::Export {}).unwrap();
    lab.edit_row("receipts", "response_json=X'00'", "operation_id='replayed'");
    let error = store.execute_with_receipt(&request).unwrap_err();
    assert!(matches!(error, DurableError::Storage(_)), "{error:?}");
    assert_failed_closed(&lab, &mut store, &error);
}

/// The stored rows of the attempt a new audit row extends, and the stored row of a
/// replayed audit event, are read at use.
#[test]
fn unreadable_audit_rows_met_at_use_close_the_store() {
    let first = prepared("attempt");
    let mut completion = first.clone();
    completion.audit_event_id = "audit-attempt-completed".into();
    completion.phase = AuditPhase::OutboundExchangeCompleted;
    completion.outcome = AuditOutcome::Unavailable;
    completion.authenticated_peer_id = None;
    completion.error_category = Some(AuditErrorCategory::Unavailable);
    completion.reason_code = Some(RefusalReason::TransportUnavailable);
    for next in [completion, first.clone()] {
        let lab = Lab::new();
        let mut store = lab.open();
        store.record_exchange_audit(&first).unwrap();
        store.record_exchange_audit(&prepared("later")).unwrap();
        store.execute(&Request::Export {}).unwrap();
        lab.edit_row(
            "exchange_audit_events",
            "record_json=X'00'",
            "audit_event_id='audit-attempt'",
        );
        let error = store.record_exchange_audit(&next).unwrap_err();
        assert!(matches!(error, DurableError::Storage(_)), "{error:?}");
        assert_failed_closed(&lab, &mut store, &error);
    }
}

/// Waiting for another connection's write lock reads no stored row: the operation
/// returns `storage` when the busy timeout expires and the store stays open.
#[test]
fn a_busy_store_is_not_closed() {
    let lab = Lab::new();
    let mut store = lab.open();
    store.record_exchange_audit(&prepared("before")).unwrap();
    let blocker = Connection::open(lab.database()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let error = store
        .record_exchange_audit(&prepared("while-busy"))
        .unwrap_err();
    assert!(matches!(error, DurableError::Storage(_)), "{error:?}");
    assert!(store.integrity().unwrap().failure.is_none());
    blocker.execute_batch("ROLLBACK").unwrap();
    store
        .record_exchange_audit(&prepared("while-busy"))
        .unwrap();
    store.execute(&Request::Export {}).unwrap();
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

/// Review of lot V2-R, probe P4: a schema found corrupt when a process opened a
/// store again closed nothing, so restoring the dropped trigger let the next open
/// serve the store. The failure is now recorded whether the open's own
/// transaction or its read-only preflight finds it.
#[test]
fn a_schema_found_corrupt_at_open_closes_the_store() {
    // The process keeps the store open: the next open trusts the file state it
    // already preflighted, and its own transaction checks the schema.
    let lab = Lab::new();
    let mut kept = lab.open();
    kept.record_exchange_audit(&prepared("one")).unwrap();
    let connection = Connection::open(lab.database()).unwrap();
    connection
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    assert_eq!(
        Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        schema_mismatch()
    );
    connection.execute_batch(AUDIT_NO_UPDATE_TRIGGER).unwrap();
    assert_failed_closed(&lab, &mut kept, &schema_mismatch());

    // Nothing keeps the store open, and the checkpoint of the last connection
    // changed the file: the next open copies it for its preflight, which checks
    // the schema before SQLite opens the store read-write.
    let lab = Lab::new();
    lab.open().record_exchange_audit(&prepared("one")).unwrap();
    Connection::open(lab.database())
        .unwrap()
        .execute_batch("DROP TRIGGER exchange_audit_events_no_update")
        .unwrap();
    assert_eq!(
        Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        schema_mismatch()
    );
    Connection::open(lab.database())
        .unwrap()
        .execute_batch(AUDIT_NO_UPDATE_TRIGGER)
        .unwrap();
    assert_eq!(
        Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        schema_mismatch()
    );
}

/// Probe P5: the closed state belongs to a database file, not to its path. A
/// clean copy renamed over the path is another database file, which the process
/// verifies completely at its first open before serving it; the closed file stays
/// closed for the handles that still have it open.
#[test]
fn another_database_file_at_a_closed_store_path_is_verified_completely_before_it_serves() {
    let lab = Lab::new();
    let mut closed = lab.open();
    closed.record_exchange_audit(&prepared("one")).unwrap();
    let copy = copy_store(&lab);
    insert_malformed_audit_row_at(&lab, 2, "forged-tail");
    let error = closed.execute(&Request::Export {}).unwrap_err();
    assert!(matches!(error, DurableError::Corrupt(_)), "{error:?}");
    assert_eq!(
        Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        error
    );

    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", lab.database().display()));
    }
    fs::rename(&copy, lab.database()).unwrap();
    assert_eq!(closed.execute(&Request::Export {}).unwrap_err(), error);
    drop(closed);
    let mut replaced = lab.open();
    let integrity = replaced.integrity().unwrap();
    assert_eq!(integrity.full_verifications, 1);
    assert!(integrity.failure.is_none());
    replaced.execute(&Request::Export {}).unwrap();
    replaced.record_exchange_audit(&prepared("two")).unwrap();
}

/// Review of lot V2-R: an operation that had passed its check of the closed
/// state still committed after another operation of the process closed the
/// store. Here the operation waits for another connection's write lock, after that
/// check, while the periodic pass of the same process finds an edited old row; it
/// then rolls back and returns that failure.
#[test]
fn no_transaction_commits_after_the_store_is_closed() {
    let lab = Lab::new();
    let mut writer = lab.open();
    for label in ["old", "newer"] {
        writer.record_exchange_audit(&prepared(label)).unwrap();
    }
    writer.execute(&Request::Export {}).unwrap();
    let mut verifier = lab.open();
    lab.edit_audit_row("audit-old");
    let rows = lab.audit_rows();

    let blocker = Connection::open(lab.database()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let pending = thread::spawn(move || writer.record_exchange_audit(&prepared("pending")).err());
    thread::sleep(Duration::from_millis(500));
    // Without the lock the operation takes milliseconds: it is waiting in BEGIN
    // IMMEDIATE, the only step that blocks, after its check of the closed state.
    assert!(!pending.is_finished());
    assert_eq!(
        verifier.verify_full().unwrap_err(),
        edited_prepared_row_error()
    );
    blocker.execute_batch("ROLLBACK").unwrap();
    assert_eq!(pending.join().unwrap(), Some(edited_prepared_row_error()));
    assert_eq!(lab.audit_rows(), rows);
}

/// The rowid rule covers every append-only table: a row that any writer adds
/// after a gap is refused by the next transaction, whichever table it is in.
#[test]
fn rows_added_after_a_gap_are_refused_in_every_table() {
    for table in ["facts", "receipts", "exchange_audit_events"] {
        let lab = Lab::new();
        let mut store = lab.open();
        store
            .execute_with_receipt(&observation("own", "value"))
            .unwrap();
        store.record_exchange_audit(&prepared("own")).unwrap();
        store.execute(&Request::Export {}).unwrap();
        if table == "exchange_audit_events" {
            insert_audit_row_at(&lab, 4, &prepared("after-a-gap"));
        } else {
            insert_row_after_a_gap(&lab, table);
        }
        let expected = rowid_gap(table);
        assert_eq!(
            store.execute(&Request::Export {}).unwrap_err(),
            expected,
            "{table}"
        );
        assert_failed_closed(&lab, &mut store, &expected);
    }
}

/// A row whose rowid alone moves keeps every checksum it had: only the rowid rule
/// of a complete verification finds the gap, in every table.
#[test]
fn a_row_moved_to_another_rowid_is_refused_by_the_complete_verification() {
    for table in ["facts", "receipts", "exchange_audit_events"] {
        let lab = Lab::new();
        let mut store = lab.open();
        for index in 0..2 {
            store
                .execute_with_receipt(&observation(&format!("own-{index}"), "value"))
                .unwrap();
            store
                .record_exchange_audit(&prepared(&format!("own-{index}")))
                .unwrap();
        }
        store.execute(&Request::Export {}).unwrap();
        lab.edit_row(
            table,
            "rowid = rowid + 5",
            &format!("rowid = (SELECT max(rowid) FROM {table})"),
        );
        let expected = rowid_gap(table);
        assert_eq!(store.verify_full().unwrap_err(), expected, "{table}");
        assert_failed_closed(&lab, &mut store, &expected);
    }
}

/// The complete verification of `--inspect-store` applies the rowid rule too,
/// which SQLite's own integrity check knows nothing about.
#[test]
fn the_full_inspection_refuses_a_rowid_gap_that_sqlite_accepts() {
    let lab = Lab::new();
    {
        let mut store = lab.open();
        for index in 0..2 {
            store
                .execute_with_receipt(&observation(&format!("own-{index}"), "value"))
                .unwrap();
            store
                .record_exchange_audit(&prepared(&format!("own-{index}")))
                .unwrap();
        }
    }
    for table in ["facts", "receipts", "exchange_audit_events"] {
        assert!(inspect_read_only(&lab.database(), &configuration(), "r1").is_ok());
        lab.edit_row(
            table,
            "rowid = rowid + 5",
            &format!("rowid = (SELECT max(rowid) FROM {table})"),
        );
        let connection = Connection::open(lab.database()).unwrap();
        let sqlite: String = connection
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .unwrap();
        drop(connection);
        assert_eq!(sqlite, "ok");
        assert_eq!(
            inspect_read_only(&lab.database(), &configuration(), "r1").unwrap_err(),
            rowid_gap(table)
        );
        lab.edit_row(
            table,
            "rowid = rowid - 5",
            &format!("rowid = (SELECT max(rowid) FROM {table})"),
        );
    }
}

/// The stored rows an authenticated import reads before it commits — the audit
/// event it may replay and the rows of its attempt — are read at use: bytes that
/// cannot be read there close the store.
#[test]
fn unreadable_audit_rows_met_by_an_authenticated_import_close_the_store() {
    let lab = Lab::new();
    let peer = other_replica_store(&lab);
    let Response::Snapshot { snapshot } = Store::open(&peer, configuration(), "r2")
        .unwrap()
        .execute(&Request::Export {})
        .unwrap()
    else {
        panic!("export returned another response");
    };
    let mut store = lab.open();
    store
        .record_exchange_audit(&inbound_observed("import", "peer-sync"))
        .unwrap();
    // A later row, so that the row this import reads is not the last verified one.
    store.record_exchange_audit(&prepared("later")).unwrap();
    store.execute(&Request::Export {}).unwrap();
    lab.edit_row(
        "exchange_audit_events",
        "record_json=X'00'",
        "audit_event_id='audit-import-observed'",
    );

    let error = store
        .execute_authenticated_import(
            "peer-sync",
            &snapshot,
            inbound_import("import", "peer-sync"),
        )
        .unwrap_err();
    assert!(matches!(error, DurableError::Storage(_)), "{error:?}");
    assert_failed_closed(&lab, &mut store, &error);
}

/// A database file this process closed is refused before anything reads it: no
/// copy of it is made for a preflight. The file is unreadable here, so a copy
/// would fail with another error.
#[test]
fn a_closed_database_file_is_refused_before_it_is_read() {
    let lab = Lab::new();
    let expected = {
        let mut store = lab.open();
        store.record_exchange_audit(&prepared("stored")).unwrap();
        store.record_exchange_audit(&prepared("next")).unwrap();
        store.execute(&Request::Export {}).unwrap();
        lab.edit_audit_row("audit-stored");
        let error = store.verify_full().unwrap_err();
        assert_eq!(error, edited_prepared_row_error());
        error
    };
    // No store of this process has the file open, and its state is one this
    // process has not preflighted, so an open would copy it.
    assert!(Command::new("touch")
        .args(["-m", "-d", "2020-01-01"])
        .arg(lab.database())
        .status()
        .unwrap()
        .success());
    fs::set_permissions(lab.database(), fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        expected
    );
    fs::set_permissions(lab.database(), fs::Permissions::from_mode(0o600)).unwrap();
}

/// The last verified rows an open reads are read at use: bytes that cannot be
/// read there close the store, as they do in any other transaction.
#[test]
fn unreadable_last_verified_rows_met_by_an_open_close_the_store() {
    let lab = Lab::new();
    let mut kept = lab.open();
    kept.record_exchange_audit(&prepared("first")).unwrap();
    kept.record_exchange_audit(&prepared("last")).unwrap();
    kept.execute(&Request::Export {}).unwrap();
    lab.edit_row(
        "exchange_audit_events",
        "sha256=X'00'",
        "audit_event_id='audit-last'",
    );

    let error = Store::open(&lab.database(), configuration(), "r1")
        .err()
        .unwrap();
    assert!(matches!(error, DurableError::Storage(_)), "{error:?}");
    assert_eq!(kept.integrity().unwrap().failure, Some(error.clone()));
    assert_failed_closed(&lab, &mut kept, &error);
}

/// The facts-only inspection returns the facts of the full inspection, verified,
/// without reading a receipt or an audit row.
#[test]
fn facts_only_inspection_verifies_the_facts_and_reads_no_audit_row() {
    let lab = Lab::new();
    {
        let mut store = lab.open();
        for index in 0..3 {
            store
                .execute_with_receipt(&observation(&format!("fact-{index}"), "value"))
                .unwrap();
        }
        for label in ["one", "two"] {
            store.record_exchange_audit(&prepared(label)).unwrap();
        }
    }
    let full = inspect_read_only(&lab.database(), &configuration(), "r1").unwrap();
    let facts = inspect_facts_read_only(&lab.database(), &configuration(), "r1").unwrap();
    assert_eq!(facts.history_count, 3);
    assert_eq!(facts.history_count, full.history_count);
    assert_eq!(facts.ordered_facts, full.ordered_facts);
    assert_eq!(facts.logical_history_sha256, full.logical_history_sha256);

    // An audit row that the full inspection refuses is not read.
    insert_malformed_audit_row_at(&lab, 3, "forged");
    assert!(matches!(
        inspect_read_only(&lab.database(), &configuration(), "r1"),
        Err(DurableError::Corrupt(_))
    ));
    assert_eq!(
        inspect_facts_read_only(&lab.database(), &configuration(), "r1").unwrap(),
        facts
    );

    // The facts table keeps the rowid rule, and every fact is verified.
    let connection = Connection::open(lab.database()).unwrap();
    connection
        .execute(
            "INSERT INTO facts(rowid, event_id, fact_json, sha256) VALUES (0, 'forged-fact', '{}', 'x')",
            [],
        )
        .unwrap();
    assert_eq!(
        inspect_facts_read_only(&lab.database(), &configuration(), "r1").unwrap_err(),
        rowid_gap("facts")
    );
    let lab = Lab::new();
    lab.open()
        .execute_with_receipt(&observation("fact", "value"))
        .unwrap();
    Connection::open(lab.database())
        .unwrap()
        .execute(
            "INSERT INTO facts(event_id, fact_json, sha256) VALUES ('r1:00000000000000000002', '{}', 'not the checksum')",
            [],
        )
        .unwrap();
    assert_eq!(
        inspect_facts_read_only(&lab.database(), &configuration(), "r1").unwrap_err(),
        DurableError::Corrupt("stored fact hash mismatch".into())
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

/// While this process has a store open, no open of it copies its files, whatever
/// changed them: reading them around SQLite would release the locks of the
/// connections this process holds on them. The child runs with a temporary
/// directory that does not exist, so any copy fails.
#[test]
fn a_store_this_process_has_open_is_never_copied_again() {
    let lab = Lab::new();
    // The child's temporary directory does not exist, so any read-only preflight
    // copy of an existing store fails instead of silently copying the store.
    let output = Command::new(env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "preflight_cache_worker",
            "--nocapture",
        ])
        .env(
            "TMPDIR",
            lab.directory.path().join("missing-temporary-directory"),
        )
        .env("MANAGER_INTEGRITY_STORE", lab.database())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "invoked as a child process by a_store_this_process_has_open_is_never_copied_again"]
fn preflight_cache_worker() {
    let path = PathBuf::from(env::var_os("MANAGER_INTEGRITY_STORE").unwrap());
    let state = |path: &Path| {
        let metadata = fs::metadata(path).unwrap();
        (metadata.len(), metadata.mtime(), metadata.mtime_nsec())
    };
    // A new file needs no preflight; this store stays open like a resident's.
    let mut store = Store::open(&path, configuration(), "r1").unwrap();
    let before = state(&path);
    for index in 0..600 {
        store
            .record_exchange_audit(&prepared(&format!("checkpoint-{index}")))
            .unwrap();
    }
    assert_ne!(
        state(&path),
        before,
        "no checkpoint changed the database file"
    );
    // The file changed through this process's own commits and checkpoints: later
    // opens copy nothing.
    for index in 0..3 {
        let mut again = Store::open(&path, configuration(), "r1").unwrap();
        again.execute(&Request::Export {}).unwrap();
        again
            .execute_with_receipt(&observation(&format!("reopened-{index}"), "value"))
            .unwrap();
    }
    // A checkpoint by another connection changes the file outside the Store; while
    // this process has the store open, its next open copies nothing either.
    store
        .record_exchange_audit(&prepared("pending-frames"))
        .unwrap();
    let trusted = state(&path);
    Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    assert_ne!(
        state(&path),
        trusted,
        "the external checkpoint changed nothing"
    );
    let mut again = Store::open(&path, configuration(), "r1").unwrap();
    again.execute(&Request::Export {}).unwrap();

    // An inspection of a store this process has open reads it through SQLite,
    // in one read transaction, rather than copying it.
    let inspected = inspect_read_only(&path, &configuration(), "r1").unwrap();
    assert_eq!(inspected.history_count, 3);
    assert_eq!(
        inspect_facts_read_only(&path, &configuration(), "r1")
            .unwrap()
            .ordered_facts,
        inspected.ordered_facts
    );

    // Once no store of this process has the file open, its files are copied again,
    // which cannot copy them here.
    drop(again);
    drop(store);
    assert!(matches!(
        Store::open(&path, configuration(), "r1"),
        Err(DurableError::Storage(_))
    ));
    assert!(matches!(
        inspect_read_only(&path, &configuration(), "r1"),
        Err(DurableError::Storage(_))
    ));
}

/// Another database file at the path of a store this process has open is refused,
/// and nothing of this process reads or opens it: its sidecars may be those of the
/// file that is open.
#[test]
fn another_file_at_the_path_of_an_open_store_is_refused_untouched() {
    let lab = Lab::new();
    let mut kept = lab.open();
    kept.record_exchange_audit(&prepared("one")).unwrap();
    let copy = copy_store(&lab);
    let copy_bytes = fs::read(&copy).unwrap();
    fs::rename(&copy, lab.database()).unwrap();

    let expected = DurableError::Storage(
        "manager store path no longer names the database file this process has open".into(),
    );
    assert_eq!(
        Store::open(&lab.database(), configuration(), "r1")
            .err()
            .unwrap(),
        expected
    );
    assert_eq!(
        inspect_read_only(&lab.database(), &configuration(), "r1").unwrap_err(),
        expected
    );
    assert_eq!(fs::read(lab.database()).unwrap(), copy_bytes);
    // The store this process has open keeps serving from the file it opened.
    kept.record_exchange_audit(&prepared("two")).unwrap();
    assert!(kept.integrity().unwrap().failure.is_none());
}
