//! The cost of one store operation must not grow with the exchange audit table.
//!
//! A child process seeds a store with realistic exchange attempts: one
//! authenticated import made through the Store API, then rows written directly
//! with exactly the record JSON, checksum and receipt links the Store writes.
//! The parent's first open verifies every seeded row, so a row the Store would
//! refuse fails the test. The parent then times the resident's store work at
//! 1,000 and 30,000 audit rows.
//!
//! The stores live in a memory-backed directory when one is available, so the
//! timings measure work that depends on the table rather than the device's
//! per-commit fsync latency; `PODMESH_MANAGER_BENCH_DIR` selects another
//! directory. `PODMESH_MANAGER_BENCH_REPETITIONS` and
//! `PODMESH_MANAGER_BENCH_REPORT_ONLY=1` exist for manual measurement runs.
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use podmesh_manager_ha_lab::{
    durable::{
        authenticated_import_receipt_id, AuditDirection, AuditErrorCategory, AuditOutcome,
        AuditPhase, Configuration, ExchangeAuditEvent, ReceiptEvidence, RefusalReason, Request,
        Response, Snapshot, Store,
    },
    ReplicaConfig, ScopeGrant,
};
use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

const SMALL_AUDIT_ROWS: usize = 1_000;
const LARGE_AUDIT_ROWS: usize = 30_000;
const REQUEST_BODY_BYTES: u64 = 5_531;
const REPLY_BODY_BYTES: u64 = 693;
const SEED_WIRE_OPERATION: &str = "seed-sync";

#[test]
fn store_operation_cost_does_not_grow_with_the_audit_table() {
    let repetitions = env::var("PODMESH_MANAGER_BENCH_REPETITIONS")
        .ok()
        .map_or(25, |value| value.parse::<usize>().unwrap());
    let report_only = env::var_os("PODMESH_MANAGER_BENCH_REPORT_ONLY").is_some();
    let small = measure(SMALL_AUDIT_ROWS, repetitions);
    let large = measure(LARGE_AUDIT_ROWS, repetitions);
    for measured in [&small, &large] {
        println!(
            "bounded-cost {} audit rows: first open {:?}; median/max append {:?}/{:?}, audit insertion {:?}/{:?}, receiver pre-reply {:?}/{:?}, export {:?}/{:?}",
            measured.audit_rows,
            measured.first_open,
            median(&measured.append),
            maximum(&measured.append),
            median(&measured.audit_insertion),
            maximum(&measured.audit_insertion),
            median(&measured.pre_reply),
            maximum(&measured.pre_reply),
            median(&measured.export),
            maximum(&measured.export),
        );
    }
    if report_only {
        return;
    }
    for (label, small, large, bound) in [
        (
            "append",
            &small.append,
            &large.append,
            Duration::from_millis(100),
        ),
        (
            "audit insertion",
            &small.audit_insertion,
            &large.audit_insertion,
            Duration::from_millis(50),
        ),
        (
            "receiver pre-reply",
            &small.pre_reply,
            &large.pre_reply,
            Duration::from_millis(200),
        ),
        (
            "export",
            &small.export,
            &large.export,
            Duration::from_millis(100),
        ),
    ] {
        let (small, large) = (median(small), median(large));
        assert!(
            small < bound && large < bound,
            "{label}: medians {small:?} at {SMALL_AUDIT_ROWS} rows and {large:?} at {LARGE_AUDIT_ROWS} rows exceed {bound:?}"
        );
        assert!(
            large <= small * 3 + Duration::from_millis(10),
            "{label}: median grew from {small:?} at {SMALL_AUDIT_ROWS} rows to {large:?} at {LARGE_AUDIT_ROWS} rows"
        );
    }
}

struct Measured {
    audit_rows: usize,
    first_open: Duration,
    append: Vec<Duration>,
    audit_insertion: Vec<Duration>,
    pre_reply: Vec<Duration>,
    export: Vec<Duration>,
}

fn measure(audit_rows: usize, repetitions: usize) -> Measured {
    let directory = bench_directory();
    let seeding = Command::new(env::current_exe().unwrap())
        .args(["--ignored", "--exact", "seed_audit_store", "--nocapture"])
        .env("PODMESH_MANAGER_BENCH_SEED_DIRECTORY", directory.path())
        .env("PODMESH_MANAGER_BENCH_SEED_ROWS", audit_rows.to_string())
        .output()
        .unwrap();
    assert!(
        seeding.status.success(),
        "seeding child failed: {}{}",
        String::from_utf8_lossy(&seeding.stdout),
        String::from_utf8_lossy(&seeding.stderr)
    );
    let r1 = directory.path().join("r1.sqlite");
    let r2 = directory.path().join("r2.sqlite");
    let peer_snapshot = snapshot(&r2, "r2");

    let started = Instant::now();
    let mut store = Store::open(&r1, configuration(), "r1").unwrap();
    let first_open = started.elapsed();
    let stored_rows: usize = Connection::open(&r1)
        .unwrap()
        .query_row("SELECT count(*) FROM exchange_audit_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(stored_rows >= audit_rows);

    let mut measured = Measured {
        audit_rows: stored_rows,
        first_open,
        append: Vec::new(),
        audit_insertion: Vec::new(),
        pre_reply: Vec::new(),
        export: Vec::new(),
    };
    for repetition in 0..repetitions {
        // The resident's control append worker opens a store and appends.
        let started = Instant::now();
        let mut appender = Store::open(&r1, configuration(), "r1").unwrap();
        appender
            .execute_with_receipt(&Request::Observe {
                operation_id: format!("bench-append-{repetition}"),
                scope: "scope1".into(),
                subject: "bench".into(),
                exclusive_resource: None,
                active_claim: false,
                value: format!("value-{repetition}"),
            })
            .unwrap();
        measured.append.push(started.elapsed());
        drop(appender);

        // One audit insertion on an open store, then its terminal row.
        let label = format!("bench-outbound-{repetition}");
        let [prepared, completed] = outbound_unavailable(&label);
        let started = Instant::now();
        store.record_exchange_audit(&prepared).unwrap();
        measured.audit_insertion.push(started.elapsed());
        store.record_exchange_audit(&completed).unwrap();

        // The resident's incoming worker opens a store and records everything
        // that precedes its first reply byte; replays alternate with new wire
        // operations, as in a replica's steady state.
        let wire_operation = if repetition % 2 == 0 {
            SEED_WIRE_OPERATION.to_string()
        } else {
            format!("bench-sync-{repetition}")
        };
        let label = format!("bench-inbound-{repetition}");
        let started = Instant::now();
        let mut receiver = Store::open(&r1, configuration(), "r1").unwrap();
        receiver
            .record_exchange_audit(&inbound_observed(&label, &wire_operation))
            .unwrap();
        let imported = receiver
            .execute_authenticated_import(
                &wire_operation,
                &peer_snapshot,
                inbound_import(&label, &wire_operation),
            )
            .unwrap();
        let receipt = imported.executed.receipt.clone().unwrap();
        receiver
            .record_exchange_audit(&inbound_reply_prepared(
                &label,
                &wire_operation,
                &receipt,
                imported.executed.replayed,
            ))
            .unwrap();
        measured.pre_reply.push(started.elapsed());
        receiver
            .record_exchange_audit(&inbound_reply_written(
                &label,
                &wire_operation,
                &receipt,
                imported.executed.replayed,
            ))
            .unwrap();
        drop(receiver);

        // The resident's synchronizer opens a store and exports its snapshot.
        let started = Instant::now();
        let mut exporter = Store::open(&r1, configuration(), "r1").unwrap();
        let Response::Snapshot { .. } = exporter.execute(&Request::Export {}).unwrap() else {
            panic!("export returned another response");
        };
        measured.export.push(started.elapsed());
    }
    measured
}

/// Seeding child: an authenticated import through the Store API, then complete
/// exchange attempts written with the Store's own record format.
#[test]
#[ignore = "invoked as the seeding child process of the bounded-cost test"]
fn seed_audit_store() {
    let directory = PathBuf::from(env::var_os("PODMESH_MANAGER_BENCH_SEED_DIRECTORY").unwrap());
    let target: usize = env::var("PODMESH_MANAGER_BENCH_SEED_ROWS")
        .unwrap()
        .parse()
        .unwrap();
    let r1 = directory.join("r1.sqlite");
    let r2 = directory.join("r2.sqlite");
    Store::open(&r2, configuration(), "r2")
        .unwrap()
        .execute(&Request::Observe {
            operation_id: "peer-fact".into(),
            scope: "scope2".into(),
            subject: "peer".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "peer value".into(),
        })
        .unwrap();
    let peer_snapshot = snapshot(&r2, "r2");
    let receipt = {
        let mut store = Store::open(&r1, configuration(), "r1").unwrap();
        let label = "seed-api-import";
        store
            .record_exchange_audit(&inbound_observed(label, SEED_WIRE_OPERATION))
            .unwrap();
        let imported = store
            .execute_authenticated_import(
                SEED_WIRE_OPERATION,
                &peer_snapshot,
                inbound_import(label, SEED_WIRE_OPERATION),
            )
            .unwrap();
        let receipt = imported.executed.receipt.unwrap();
        store
            .record_exchange_audit(&inbound_reply_prepared(
                label,
                SEED_WIRE_OPERATION,
                &receipt,
                false,
            ))
            .unwrap();
        store
            .record_exchange_audit(&inbound_reply_written(
                label,
                SEED_WIRE_OPERATION,
                &receipt,
                false,
            ))
            .unwrap();
        receipt
    };
    let mut connection = Connection::open(&r1).unwrap();
    let transaction = connection.transaction().unwrap();
    let mut rows = 4;
    let mut round = 0;
    while rows < target {
        let label = format!("seed-{round}");
        let mut events = vec![
            inbound_observed(&label, SEED_WIRE_OPERATION),
            inbound_committed_replay(&label, &receipt),
            inbound_reply_prepared(&label, SEED_WIRE_OPERATION, &receipt, true),
            inbound_reply_written(&label, SEED_WIRE_OPERATION, &receipt, true),
        ];
        events.extend(outbound_accepted(&format!("seed-accepted-{round}")));
        events.extend(outbound_unavailable(&format!("seed-unavailable-{round}")));
        for event in &events {
            insert_audit_row(&transaction, event);
        }
        rows += events.len();
        round += 1;
    }
    transaction.commit().unwrap();
    connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
}

fn bench_directory() -> tempfile::TempDir {
    let base = env::var_os("PODMESH_MANAGER_BENCH_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let memory = Path::new("/dev/shm");
            memory.is_dir().then(|| memory.to_path_buf())
        });
    base.and_then(|base| {
        tempfile::Builder::new()
            .prefix("podmesh-manager-bench-")
            .tempdir_in(base)
            .ok()
    })
    .unwrap_or_else(|| tempfile::tempdir().unwrap())
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

fn snapshot(path: &Path, replica: &str) -> Snapshot {
    let Response::Snapshot { snapshot } = Store::open(path, configuration(), replica)
        .unwrap()
        .execute(&Request::Export {})
        .unwrap()
    else {
        panic!("export returned another response");
    };
    snapshot
}

fn digest(label: &str) -> String {
    format!("{:x}", Sha256::digest(label.as_bytes()))
}

/// One phase of an exchange attempt, shaped like the network crate's events.
fn event(
    label: &str,
    direction: AuditDirection,
    phase: AuditPhase,
    authenticated: bool,
    operation: &str,
    outcome: AuditOutcome,
) -> ExchangeAuditEvent {
    let (error_category, reason_code) = match outcome {
        AuditOutcome::Unavailable => (
            Some(AuditErrorCategory::Unavailable),
            Some(RefusalReason::TransportUnavailable),
        ),
        _ => (None, None),
    };
    ExchangeAuditEvent {
        audit_event_id: format!("audit-{}", digest(&format!("{label}:{phase:?}"))),
        attempt_id: format!("attempt:{}", digest(label)),
        wire_nonce: format!("nonce-{}", &digest(label)[..32]),
        direction,
        phase,
        authenticated_peer_id: authenticated.then(|| "r2".into()),
        peer_claim: Some("r2".into()),
        operation_id: Some(operation.into()),
        request_frame_bytes: 0,
        request_announced_body_bytes: None,
        request_sha256: None,
        reply_frame_bytes: 0,
        reply_announced_body_bytes: None,
        reply_sha256: None,
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

fn with_request(
    mut event: ExchangeAuditEvent,
    label: &str,
    transferred: bool,
) -> ExchangeAuditEvent {
    event.request_frame_bytes = if transferred {
        REQUEST_BODY_BYTES + 4
    } else {
        0
    };
    event.request_announced_body_bytes = Some(REQUEST_BODY_BYTES);
    event.request_sha256 = Some(digest(&format!("request:{label}")));
    event
}

fn with_reply(mut event: ExchangeAuditEvent, label: &str, transferred: bool) -> ExchangeAuditEvent {
    event.reply_frame_bytes = if transferred { REPLY_BODY_BYTES + 4 } else { 0 };
    event.reply_announced_body_bytes = Some(REPLY_BODY_BYTES);
    event.reply_sha256 = Some(digest(&format!("reply:{label}")));
    event
}

fn with_local_receipt(
    mut event: ExchangeAuditEvent,
    receipt: &ReceiptEvidence,
    replayed: bool,
) -> ExchangeAuditEvent {
    event.local_receipt_operation_id = Some(receipt.operation_id.clone());
    event.local_receipt_sha256 = Some(receipt.sha256.clone());
    event.replayed = replayed;
    event
}

fn inbound_observed(label: &str, operation: &str) -> ExchangeAuditEvent {
    with_request(
        event(
            label,
            AuditDirection::Inbound,
            AuditPhase::InboundRequestObserved,
            true,
            operation,
            AuditOutcome::Accepted,
        ),
        label,
        true,
    )
}

fn inbound_import(label: &str, operation: &str) -> ExchangeAuditEvent {
    event(
        label,
        AuditDirection::Inbound,
        AuditPhase::InboundImportCommitted,
        true,
        operation,
        AuditOutcome::Accepted,
    )
}

fn inbound_committed_replay(label: &str, receipt: &ReceiptEvidence) -> ExchangeAuditEvent {
    with_local_receipt(inbound_import(label, SEED_WIRE_OPERATION), receipt, true)
}

fn inbound_reply_prepared(
    label: &str,
    operation: &str,
    receipt: &ReceiptEvidence,
    replayed: bool,
) -> ExchangeAuditEvent {
    with_local_receipt(
        with_reply(
            event(
                label,
                AuditDirection::Inbound,
                AuditPhase::InboundReplyPrepared,
                true,
                operation,
                AuditOutcome::Incomplete,
            ),
            label,
            false,
        ),
        receipt,
        replayed,
    )
}

fn inbound_reply_written(
    label: &str,
    operation: &str,
    receipt: &ReceiptEvidence,
    replayed: bool,
) -> ExchangeAuditEvent {
    with_local_receipt(
        with_reply(
            event(
                label,
                AuditDirection::Inbound,
                AuditPhase::InboundReplyWriteObserved,
                true,
                operation,
                AuditOutcome::Accepted,
            ),
            label,
            true,
        ),
        receipt,
        replayed,
    )
}

fn outbound_prepared(label: &str, operation: &str) -> ExchangeAuditEvent {
    with_request(
        event(
            label,
            AuditDirection::Outbound,
            AuditPhase::OutboundRequestPrepared,
            true,
            operation,
            AuditOutcome::Incomplete,
        ),
        label,
        false,
    )
}

fn outbound_accepted(label: &str) -> [ExchangeAuditEvent; 2] {
    let operation = format!("operation-{}", &digest(label)[..24]);
    let mut completed = with_reply(
        with_request(
            event(
                label,
                AuditDirection::Outbound,
                AuditPhase::OutboundExchangeCompleted,
                true,
                &operation,
                AuditOutcome::Accepted,
            ),
            label,
            true,
        ),
        label,
        true,
    );
    completed.remote_receipt_operation_id = Some(
        authenticated_import_receipt_id(&configuration().topology().unwrap(), "r1", &operation)
            .unwrap(),
    );
    completed.remote_receipt_sha256 = Some(digest(&format!("remote-receipt:{label}")));
    completed.replayed = true;
    [outbound_prepared(label, &operation), completed]
}

fn outbound_unavailable(label: &str) -> [ExchangeAuditEvent; 2] {
    let operation = format!("operation-{}", &digest(label)[..24]);
    let completed = with_request(
        event(
            label,
            AuditDirection::Outbound,
            AuditPhase::OutboundExchangeCompleted,
            false,
            &operation,
            AuditOutcome::Unavailable,
        ),
        label,
        false,
    );
    [outbound_prepared(label, &operation), completed]
}

fn text(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn insert_audit_row(connection: &Connection, event: &ExchangeAuditEvent) {
    let record_json = serde_json::to_string(event).unwrap();
    let checksum = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&("podmesh-manager-ha-exchange-audit/1", &record_json)).unwrap()
        )
    );
    let size = |value: Option<u64>| value.map(|value| i64::try_from(value).unwrap());
    connection
        .execute(
            "INSERT INTO exchange_audit_events (
                audit_event_id, attempt_id, wire_nonce, direction, phase, authenticated_peer_id,
                peer_claim, operation_id, request_frame_bytes,
                request_announced_body_bytes, request_sha256, reply_frame_bytes,
                reply_announced_body_bytes, reply_sha256, outcome, error_category, reason_code,
                local_receipt_operation_id, local_receipt_sha256,
                remote_receipt_operation_id, remote_receipt_sha256, replayed,
                record_json, sha256
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            params![
                event.audit_event_id,
                event.attempt_id,
                event.wire_nonce,
                text(&event.direction),
                text(&event.phase),
                event.authenticated_peer_id,
                event.peer_claim,
                event.operation_id,
                i64::try_from(event.request_frame_bytes).unwrap(),
                size(event.request_announced_body_bytes),
                event.request_sha256,
                i64::try_from(event.reply_frame_bytes).unwrap(),
                size(event.reply_announced_body_bytes),
                event.reply_sha256,
                text(&event.outcome),
                event.error_category.map(|value| text(&value)),
                event.reason_code.map(|value| text(&value)),
                event.local_receipt_operation_id,
                event.local_receipt_sha256,
                event.remote_receipt_operation_id,
                event.remote_receipt_sha256,
                i64::from(event.replayed),
                record_json,
                checksum,
            ],
        )
        .unwrap();
}

fn median(values: &[Duration]) -> Duration {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted[sorted.len() / 2]
}

fn maximum(values: &[Duration]) -> Duration {
    values.iter().copied().max().unwrap_or_default()
}
