//! Exchange-audit mutations on [`podmesh::store::Transaction`] for MariaDB journals.

use podmesh::store::{Row, Transaction, Value};
use serde::Serialize;

use super::{
    audit_digest, audit_enum_from_text, check_audit_receipt_link, enum_from_text, enum_text, error,
    json, local_receipt_reference, replay_existing_audit, validate_candidate_audit,
    validate_candidate_audit_sequence, verify_audit_sequences, verify_stored_audit_evidence,
    DurableError, DurableResult, ExchangeAuditEvent,
    ExchangeAuditEvidence, ReceiptMetadata, Topology, AUDIT_COLUMNS,
};

fn store_error(problem: podmesh::store::StoreError) -> DurableError {
    DurableError::Storage(problem.to_string())
}

fn optional_text(row: &Row, index: usize) -> DurableResult<Option<String>> {
    match row.value(index).map_err(store_error)? {
        Value::Null => Ok(None),
        _ => Ok(Some(row.text(index).map_err(store_error)?.to_string())),
    }
}

fn decode_audit_row_from_store(row: &Row) -> DurableResult<(ExchangeAuditEvent, String, String)> {
    let replayed = row.integer(21).map_err(store_error)?;
    let event = ExchangeAuditEvent {
        audit_event_id: row.text(0).map_err(store_error)?.to_string(),
        attempt_id: row.text(1).map_err(store_error)?.to_string(),
        wire_nonce: row.text(2).map_err(store_error)?.to_string(),
        direction: audit_enum_from_text(row.text(3).map_err(store_error)?)?,
        phase: audit_enum_from_text(row.text(4).map_err(store_error)?)?,
        authenticated_peer_id: optional_text(row, 5)?,
        peer_claim: optional_text(row, 6)?,
        operation_id: optional_text(row, 7)?,
        request_frame_bytes: u64::try_from(row.integer(8).map_err(store_error)?)
            .map_err(|_| DurableError::Corrupt("stored audit byte count is negative".into()))?,
        request_announced_body_bytes: match row.value(9).map_err(store_error)? {
            Value::Null => None,
            _ => Some(
                u64::try_from(row.integer(9).map_err(store_error)?).map_err(|_| {
                    DurableError::Corrupt("stored audit byte count is negative".into())
                })?,
            ),
        },
        request_sha256: optional_text(row, 10)?,
        reply_frame_bytes: u64::try_from(row.integer(11).map_err(store_error)?)
            .map_err(|_| DurableError::Corrupt("stored audit byte count is negative".into()))?,
        reply_announced_body_bytes: match row.value(12).map_err(store_error)? {
            Value::Null => None,
            _ => Some(
                u64::try_from(row.integer(12).map_err(store_error)?).map_err(|_| {
                    DurableError::Corrupt("stored audit byte count is negative".into())
                })?,
            ),
        },
        reply_sha256: optional_text(row, 13)?,
        outcome: audit_enum_from_text(row.text(14).map_err(store_error)?)?,
        error_category: optional_text(row, 15)?
            .map(|value| audit_enum_from_text(&value))
            .transpose()?,
        reason_code: optional_text(row, 16)?
            .map(|value| audit_enum_from_text(&value))
            .transpose()?,
        local_receipt_operation_id: optional_text(row, 17)?,
        local_receipt_sha256: optional_text(row, 18)?,
        remote_receipt_operation_id: optional_text(row, 19)?,
        remote_receipt_sha256: optional_text(row, 20)?,
        replayed: match replayed {
            0 => false,
            1 => true,
            _ => {
                return Err(DurableError::Corrupt(
                    "stored audit replay flag is invalid".into(),
                ));
            }
        },
    };
    let record_json = row.text(22).map_err(store_error)?.to_string();
    let sha256 = row.text(23).map_err(store_error)?.to_string();
    Ok((event, record_json, sha256))
}

fn receipt_metadata_transaction(
    transaction: &mut dyn Transaction,
    operation_id: &str,
    sha256: &str,
) -> DurableResult<Option<ReceiptMetadata>> {
    let rows = transaction
        .query(
            "SELECT kind, source_replica_id, wire_operation_id FROM receipts WHERE operation_id = ? AND sha256 = ?",
            &[Value::from(operation_id), Value::from(sha256)],
        )
        .map_err(store_error)?;
    rows.first()
        .map(|row| {
            Ok(ReceiptMetadata {
                kind: enum_from_text(row.text(0).map_err(store_error)?)?,
                source_replica_id: optional_text(row, 1)?,
                wire_operation_id: optional_text(row, 2)?,
            })
        })
        .transpose()
}

fn verify_audit_receipt_link_transaction(
    transaction: &mut dyn Transaction,
    event: &ExchangeAuditEvent,
) -> DurableResult<()> {
    let Some((operation_id, sha256)) = local_receipt_reference(event) else {
        return Ok(());
    };
    check_audit_receipt_link(
        event,
        receipt_metadata_transaction(transaction, operation_id, sha256)?,
    )
}

fn query_verified_audits_transaction(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
    sql: &str,
    parameters: &[Value],
) -> DurableResult<Vec<ExchangeAuditEvidence>> {
    let rows = transaction.query(sql, parameters).map_err(store_error)?;
    let mut evidence = Vec::new();
    for row in rows {
        let (event, record_json, sha256) = decode_audit_row_from_store(&row)?;
        evidence.push(verify_stored_audit_evidence(
            topology,
            replica_id,
            event,
            &record_json,
            sha256,
            |event| {
                verify_audit_receipt_link_transaction(transaction, event).map_err(|problem| {
                    DurableError::Corrupt(format!("stored audit receipt link is invalid: {problem}"))
                })
            },
        )?);
    }
    Ok(evidence)
}

fn load_audit_by_id_transaction(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
    audit_event_id: &str,
) -> DurableResult<Option<ExchangeAuditEvidence>> {
    Ok(query_verified_audits_transaction(
        transaction,
        topology,
        replica_id,
        &format!("SELECT {AUDIT_COLUMNS} FROM exchange_audit_events WHERE audit_event_id = ?"),
        &[Value::from(audit_event_id)],
    )?
    .pop())
}

fn load_attempt_audits_transaction(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
    attempt_id: &str,
) -> DurableResult<Vec<ExchangeAuditEvidence>> {
    let evidence = query_verified_audits_transaction(
        transaction,
        topology,
        replica_id,
        &format!(
            "SELECT {AUDIT_COLUMNS} FROM exchange_audit_events
             WHERE direction IN ('inbound', 'outbound') AND attempt_id = ?
             ORDER BY audit_event_id"
        ),
        &[Value::from(attempt_id)],
    )?;
    verify_audit_sequences(&evidence)?;
    Ok(evidence)
}

/// Prevalidates an inbound-import audit before the import receipt exists.
pub(super) fn prevalidate_authenticated_import_audit(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
    audit: &ExchangeAuditEvent,
    local_operation_id: &str,
) -> DurableResult<()> {
    let mut candidate = audit.clone();
    candidate.local_receipt_operation_id = Some(local_operation_id.into());
    candidate.local_receipt_sha256 = Some(format!("{:064x}", 0));
    validate_candidate_audit(topology, replica_id, &candidate)?;
    if let Some(existing) = load_audit_by_id_transaction(
        transaction,
        topology,
        replica_id,
        &candidate.audit_event_id,
    )? {
        let mut comparable = existing.event;
        comparable
            .local_receipt_operation_id
            .clone_from(&candidate.local_receipt_operation_id);
        comparable
            .local_receipt_sha256
            .clone_from(&candidate.local_receipt_sha256);
        comparable.replayed = candidate.replayed;
        if comparable != candidate {
            return Err(DurableError::InvalidAudit(
                "audit event ID reused with different bytes".into(),
            ));
        }
    } else {
        let attempt = load_attempt_audits_transaction(
            transaction,
            topology,
            replica_id,
            &candidate.attempt_id,
        )?;
        validate_candidate_audit_sequence(&attempt, &candidate)?;
    }
    Ok(())
}

fn optional_enum_text<T: Serialize>(value: Option<T>) -> DurableResult<Option<String>> {
    value.map(|inner| enum_text(&inner)).transpose()
}

/// Inserts one exchange-audit row in the active journal transaction.
pub(super) fn insert_audit(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
    event: &ExchangeAuditEvent,
    allow_atomic_import_replay_normalization: bool,
) -> DurableResult<ExchangeAuditEvidence> {
    validate_candidate_audit(topology, replica_id, event)?;
    let record_json = json(event)?;
    let sha256 = audit_digest(&record_json)?;
    let prior = load_audit_by_id_transaction(
        transaction,
        topology,
        replica_id,
        &event.audit_event_id,
    )?;
    if let Some(evidence) = replay_existing_audit(
        prior,
        event,
        &record_json,
        &sha256,
        allow_atomic_import_replay_normalization,
    )? {
        return Ok(evidence);
    }
    let attempt = load_attempt_audits_transaction(
        transaction,
        topology,
        replica_id,
        &event.attempt_id,
    )?;
    validate_candidate_audit_sequence(&attempt, event)?;
    if let Some((operation_id, receipt_sha256)) = local_receipt_reference(event) {
        let receipt = receipt_metadata_transaction(transaction, operation_id, receipt_sha256)?;
        check_audit_receipt_link(event, receipt)?;
    }
    let inserted = transaction
        .execute(
            "INSERT INTO exchange_audit_events (
                audit_event_id, attempt_id, wire_nonce, direction, phase, authenticated_peer_id,
                peer_claim, operation_id, request_frame_bytes,
                request_announced_body_bytes, request_sha256, reply_frame_bytes,
                reply_announced_body_bytes, reply_sha256, outcome, error_category, reason_code,
                local_receipt_operation_id, local_receipt_sha256,
                remote_receipt_operation_id, remote_receipt_sha256, replayed,
                record_json, sha256
             ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
             )",
            &[
                Value::from(&event.audit_event_id),
                Value::from(&event.attempt_id),
                Value::from(&event.wire_nonce),
                Value::from(&enum_text(&event.direction)?),
                Value::from(&enum_text(&event.phase)?),
                event
                    .authenticated_peer_id
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                event
                    .peer_claim
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                event
                    .operation_id
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                Value::Integer(i64::try_from(event.request_frame_bytes).map_err(error)?),
                event
                    .request_announced_body_bytes
                    .map(|size| i64::try_from(size).map_err(error))
                    .transpose()?
                    .map_or(Value::Null, Value::Integer),
                event
                    .request_sha256
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                Value::Integer(i64::try_from(event.reply_frame_bytes).map_err(error)?),
                event
                    .reply_announced_body_bytes
                    .map(|size| i64::try_from(size).map_err(error))
                    .transpose()?
                    .map_or(Value::Null, Value::Integer),
                event
                    .reply_sha256
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                Value::from(&enum_text(&event.outcome)?),
                optional_enum_text(event.error_category)?
                    .map_or(Value::Null, Value::from),
                optional_enum_text(event.reason_code)?
                    .map_or(Value::Null, Value::from),
                event
                    .local_receipt_operation_id
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                event
                    .local_receipt_sha256
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                event
                    .remote_receipt_operation_id
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                event
                    .remote_receipt_sha256
                    .as_deref()
                    .map_or(Value::Null, Value::from),
                Value::Integer(i64::from(event.replayed)),
                Value::from(&record_json),
                Value::from(&sha256),
            ],
        )
        .map_err(store_error)?;
    if inserted != 1 {
        return Err(DurableError::Storage(
            "audit insert did not affect exactly one row".into(),
        ));
    }
    Ok(ExchangeAuditEvidence {
        event: event.clone(),
        sha256,
    })
}
