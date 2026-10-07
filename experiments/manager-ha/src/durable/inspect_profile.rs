//! Read-only inspection through `store.json` / [`podmesh::resolve_manager_store_profile`].
//! SQLite keeps the file snapshot path; MariaDB opens a read transaction on the profile journal.

use std::path::Path;

use podmesh::store::{self, migrations, schema_version, Engine, Row, StoreConfig, Transaction, Value};

use super::{
    audit_enum_from_text, corrupt_model, decode_fact_row, enum_from_text, finish_canonical_inspection,
    inspect_sqlite_facts_read_only, inspect_sqlite_read_only, json, validate_local_configuration,
    verify_audit_sequences, verify_receipt_row, verify_stored_audit_evidence, AUDIT_COLUMNS,
    CanonicalStoreInspection, Configuration, DurableError, DurableResult, ExchangeAuditEvent,
    FactsInspection, ReceiptEvidence, ReceiptMetadata, StoredReceipt, Topology, RECEIPT_COLUMNS,
};

/// Legacy SQLite `user_version` reported by file-backed inspection.
const LEGACY_SQLITE_SCHEMA_VERSION: u32 = 3;

/// When a MariaDB profile is configured, the canonical SQLite path named in resident
/// configuration may be absent.
#[must_use]
pub fn manager_store_profile_absent_sqlite_file_ok(
    profile_dir: &Path,
    sqlite_database_path: &Path,
) -> bool {
    #[cfg(feature = "mariadb")]
    {
        podmesh::resolve_manager_store_profile(profile_dir, sqlite_database_path)
            .map(|profile| profile.engine == Engine::Mariadb)
            .unwrap_or(false)
    }
    #[cfg(not(feature = "mariadb"))]
    {
        let _ = (profile_dir, sqlite_database_path);
        false
    }
}

/// Verifies a manager store using the resolved store profile beside `profile_dir`.
///
/// # Errors
/// Refuses invalid configuration, store faults, and corrupt content.
pub fn inspect_read_only_resolved(
    profile_dir: &Path,
    sqlite_database_path: &Path,
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<CanonicalStoreInspection> {
    let profile = podmesh::resolve_manager_store_profile(profile_dir, sqlite_database_path)
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    match profile.engine {
        Engine::Sqlite => {
            inspect_sqlite_read_only(sqlite_database_path, configuration, replica_id)
        }
        Engine::Mariadb => {
            #[cfg(feature = "mariadb")]
            {
                inspect_mariadb_read_only(&profile, configuration, replica_id)
            }
            #[cfg(not(feature = "mariadb"))]
            {
                Err(DurableError::Storage(
                    "mariadb store profile requires building podmesh-manager-ha-lab with --features mariadb"
                        .into(),
                ))
            }
        }
    }
}

/// Facts-only inspection through the resolved store profile.
///
/// # Errors
/// Refuses invalid configuration, store faults, and corrupt facts.
pub fn inspect_facts_read_only_resolved(
    profile_dir: &Path,
    sqlite_database_path: &Path,
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<FactsInspection> {
    let profile = podmesh::resolve_manager_store_profile(profile_dir, sqlite_database_path)
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    match profile.engine {
        Engine::Sqlite => {
            inspect_sqlite_facts_read_only(sqlite_database_path, configuration, replica_id)
        }
        Engine::Mariadb => {
            #[cfg(feature = "mariadb")]
            {
                inspect_mariadb_facts_read_only(&profile, configuration, replica_id)
            }
            #[cfg(not(feature = "mariadb"))]
            {
                Err(DurableError::Storage(
                    "mariadb store profile requires building podmesh-manager-ha-lab with --features mariadb"
                        .into(),
                ))
            }
        }
    }
}

#[cfg(feature = "mariadb")]
fn store_error(problem: store::StoreError) -> DurableError {
    DurableError::Storage(problem.to_string())
}

#[cfg(feature = "mariadb")]
fn optional_text(row: &Row, index: usize) -> DurableResult<Option<String>> {
    match row.value(index).map_err(store_error)? {
        Value::Null => Ok(None),
        _ => Ok(Some(row.text(index).map_err(store_error)?.to_string())),
    }
}

#[cfg(feature = "mariadb")]
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

#[cfg(feature = "mariadb")]
fn verify_audit_receipt_link_transaction(
    transaction: &mut dyn Transaction,
    event: &ExchangeAuditEvent,
) -> DurableResult<()> {
    let Some((operation_id, sha256)) = super::local_receipt_reference(event) else {
        return Ok(());
    };
    super::check_audit_receipt_link(
        event,
        receipt_metadata_transaction(transaction, operation_id, sha256)?,
    )
}

#[cfg(feature = "mariadb")]
fn read_stored_receipt_row(row: &Row) -> DurableResult<StoredReceipt> {
    Ok(StoredReceipt {
        operation_id: row.text(0).map_err(store_error)?.to_string(),
        kind: row.text(1).map_err(store_error)?.to_string(),
        source_replica_id: optional_text(row, 2)?,
        wire_operation_id: optional_text(row, 3)?,
        request_json: row.text(4).map_err(store_error)?.to_string(),
        response_json: row.text(5).map_err(store_error)?.to_string(),
        sha256: row.text(6).map_err(store_error)?.to_string(),
    })
}

#[cfg(feature = "mariadb")]
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

#[cfg(feature = "mariadb")]
fn load_replica_transaction(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<super::Replica> {
    let mut replica = topology.instantiate(replica_id).map_err(corrupt_model)?;
    let rows = transaction
        .query(
            "SELECT event_id, fact_json, sha256 FROM facts ORDER BY event_id",
            &[],
        )
        .map_err(store_error)?;
    for row in rows {
        let id = row.text(0).map_err(store_error)?;
        let encoded = row.text(1).map_err(store_error)?;
        let hash = row.text(2).map_err(store_error)?;
        replica
            .ingest(decode_fact_row(id, encoded, hash)?)
            .map_err(corrupt_model)?;
    }
    Ok(replica)
}

#[cfg(feature = "mariadb")]
fn begin_mariadb_inspection(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<()> {
    let expected_topology = json(topology)?;
    let rows = transaction
        .query(
            "SELECT replica_id, topology_json FROM identity WHERE singleton = ?",
            &[Value::Integer(1)],
        )
        .map_err(store_error)?;
    if rows.is_empty() {
        return Err(DurableError::Refused(super::RefusalReason::IdentityMismatch));
    }
    let row = &rows[0];
    let held_replica = row.text(0).map_err(store_error)?;
    let held_topology = row.text(1).map_err(store_error)?;
    if held_replica != replica_id || held_topology != expected_topology {
        return Err(DurableError::Refused(super::RefusalReason::IdentityMismatch));
    }
    Ok(())
}

#[cfg(feature = "mariadb")]
fn verify_receipts_transaction(
    transaction: &mut dyn Transaction,
    logical_manager_id: &str,
) -> DurableResult<()> {
    let rows = transaction
        .query(
            &format!("SELECT {RECEIPT_COLUMNS} FROM receipts ORDER BY operation_id"),
            &[],
        )
        .map_err(store_error)?;
    for row in rows {
        verify_receipt_row(logical_manager_id, &read_stored_receipt_row(&row)?)?;
    }
    Ok(())
}

#[cfg(feature = "mariadb")]
fn load_receipt_evidence_transaction(
    transaction: &mut dyn Transaction,
) -> DurableResult<Vec<ReceiptEvidence>> {
    let rows = transaction
        .query(
            "SELECT operation_id, kind, source_replica_id, wire_operation_id, sha256 FROM receipts ORDER BY operation_id",
            &[],
        )
        .map_err(store_error)?;
    let mut evidence = Vec::new();
    for row in rows {
        evidence.push(ReceiptEvidence {
            operation_id: row.text(0).map_err(store_error)?.to_string(),
            kind: enum_from_text(row.text(1).map_err(store_error)?)?,
            source_replica_id: optional_text(&row, 2)?,
            wire_operation_id: optional_text(&row, 3)?,
            sha256: row.text(4).map_err(store_error)?.to_string(),
        });
    }
    Ok(evidence)
}

#[cfg(feature = "mariadb")]
fn load_audits_transaction(
    transaction: &mut dyn Transaction,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<Vec<super::ExchangeAuditEvidence>> {
    let rows = transaction
        .query(
            &format!("SELECT {AUDIT_COLUMNS} FROM exchange_audit_events ORDER BY audit_event_id"),
            &[],
        )
        .map_err(store_error)?;
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
    verify_audit_sequences(&evidence)?;
    Ok(evidence)
}

#[cfg(feature = "mariadb")]
fn inspect_mariadb_read_only(
    profile: &StoreConfig,
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<CanonicalStoreInspection> {
    profile
        .validate()
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    let topology = validate_local_configuration(configuration, replica_id)?;
    let opened = podmesh::open_manager_store(profile)
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    let mut store = match opened.into_journal() {
        podmesh::ManagerJournal::Durable(store) => store,
        podmesh::ManagerJournal::Sqlite(_) => {
            return Err(DurableError::Storage(
                "manager store profile named sqlite; use a file path instead".into(),
            ));
        }
    };
    let version = schema_version(store.as_mut(), migrations::MANAGER)
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    if version != Some(migrations::manager_version()) {
        return Err(DurableError::Refused(super::RefusalReason::UnsupportedSchema));
    }
    let integrity = store
        .integrity_check()
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    if !integrity.ok {
        return Err(DurableError::Corrupt(
            "MariaDB store integrity check failed".into(),
        ));
    }
    let integrity_result = integrity.result().to_string();
    let mut transaction = store.transaction().map_err(store_error)?;
    begin_mariadb_inspection(transaction.as_mut(), &topology, replica_id)?;
    verify_receipts_transaction(transaction.as_mut(), topology.logical_manager_id())?;
    let replica = load_replica_transaction(transaction.as_mut(), &topology, replica_id)?;
    let audits = load_audits_transaction(transaction.as_mut(), &topology, replica_id)?;
    let receipts = load_receipt_evidence_transaction(transaction.as_mut())?;
    let inspection = finish_canonical_inspection(
        &topology,
        replica_id,
        replica,
        audits,
        receipts,
        LEGACY_SQLITE_SCHEMA_VERSION,
        integrity_result,
    )?;
    transaction.commit().map_err(store_error)?;
    Ok(inspection)
}

#[cfg(feature = "mariadb")]
fn inspect_mariadb_facts_read_only(
    profile: &StoreConfig,
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<FactsInspection> {
    profile
        .validate()
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    let topology = validate_local_configuration(configuration, replica_id)?;
    let opened = podmesh::open_manager_store(profile)
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    let mut store = match opened.into_journal() {
        podmesh::ManagerJournal::Durable(store) => store,
        podmesh::ManagerJournal::Sqlite(_) => {
            return Err(DurableError::Storage(
                "manager store profile named sqlite; use a file path instead".into(),
            ));
        }
    };
    let version = schema_version(store.as_mut(), migrations::MANAGER)
        .map_err(|problem| DurableError::Storage(problem.to_string()))?;
    if version != Some(migrations::manager_version()) {
        return Err(DurableError::Refused(super::RefusalReason::UnsupportedSchema));
    }
    let mut transaction = store.transaction().map_err(store_error)?;
    begin_mariadb_inspection(transaction.as_mut(), &topology, replica_id)?;
    let replica = load_replica_transaction(transaction.as_mut(), &topology, replica_id)?;
    transaction.commit().map_err(store_error)?;
    let ordered_facts: Vec<_> = replica.history.into_values().collect();
    Ok(FactsInspection {
        history_count: ordered_facts.len(),
        logical_history_sha256: super::logical_history_sha256(&topology, &ordered_facts)?,
        ordered_facts,
    })
}
