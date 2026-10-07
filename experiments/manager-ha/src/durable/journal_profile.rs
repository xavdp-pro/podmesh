//! MariaDB manager journal entry path: open through `podmesh::open_manager_store` and serve the
//! laboratory request surface on [`DurableStore`]. SQLite keeps the file-backed [`Store`].

use super::{
    apply, authenticated_import_receipt_id, corrupt_model, decode_fact_row, digest,
    enum_from_text, error, json, mutation_operation_id, receipt_digest,
    receipt_metadata_for_request, store_closed_state_untracked, validate_local_configuration,
    validate_receipt_metadata, validate_receipt_operation_id, AuditDirection, AuditOutcome,
    AuditPhase, AuthenticatedImport, Configuration, DurableError, DurableResult, Executed,
    ExchangeAuditEvent, ReceiptEvidence, ReceiptKind, ReceiptMetadata, Request, Response,
    Snapshot, Store, StoreClosedState, StoreIntegrity, Topology,
};
#[cfg(feature = "mariadb")]
use super::journal_audit;
use podmesh::store::{self, Row, StoreConfig, Transaction, Value};
use podmesh::ManagerJournal;

#[allow(clippy::needless_pass_by_value)]
fn store_error(problem: store::StoreError) -> DurableError {
    DurableError::Storage(problem.to_string())
}

fn enum_text<T: serde::Serialize>(value: &T) -> DurableResult<String> {
    serde_json::to_string(value)
        .map_err(error)
        .map(|text| text.trim_matches('"').to_string())
}

fn optional_text(row: &Row, index: usize) -> DurableResult<Option<String>> {
    match row.value(index).map_err(store_error)? {
        Value::Null => Ok(None),
        _ => Ok(Some(row.text(index).map_err(store_error)?.to_string())),
    }
}

fn ensure_identity(
    store: &mut dyn store::DurableStore,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<()> {
    let expected_topology = json(topology)?;
    let rows = store
        .query(
            "SELECT replica_id, topology_json FROM identity WHERE singleton = ?",
            &[Value::Integer(1)],
        )
        .map_err(store_error)?;
    if rows.is_empty() {
        let inserted = store
            .execute(
                "INSERT INTO identity (singleton, replica_id, topology_json) VALUES (?, ?, ?)",
                &[
                    Value::Integer(1),
                    Value::from(replica_id),
                    Value::from(&expected_topology),
                ],
            )
            .map_err(store_error)?;
        if inserted != 1 {
            return Err(DurableError::Storage(
                "identity insert did not affect exactly one row".into(),
            ));
        }
        return Ok(());
    }
    let row = &rows[0];
    let held_replica = row.text(0).map_err(store_error)?;
    let held_topology = row.text(1).map_err(store_error)?;
    if held_replica != replica_id || held_topology != expected_topology {
        return Err(DurableError::Refused(super::RefusalReason::IdentityMismatch));
    }
    Ok(())
}

fn load_replica(
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

struct StoredReceipt {
    operation_id: String,
    request_json: String,
    kind: String,
    source_replica_id: Option<String>,
    wire_operation_id: Option<String>,
    response_json: String,
    sha256: String,
}

fn read_stored_receipt(row: &Row) -> DurableResult<StoredReceipt> {
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

fn stored_receipt(
    transaction: &mut dyn Transaction,
    operation_id: &str,
) -> DurableResult<Option<StoredReceipt>> {
    let rows = transaction
        .query(
            "SELECT operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256 FROM receipts WHERE operation_id = ?",
            &[Value::from(operation_id)],
        )
        .map_err(store_error)?;
    rows.first().map(read_stored_receipt).transpose()
}

fn execute_on_transaction(
    transaction: &mut dyn Transaction,
    configuration: &Configuration,
    topology: &Topology,
    replica_id: &str,
    request: &Request,
    receipt_metadata: ReceiptMetadata,
) -> DurableResult<Executed> {
    let request_json = json(request)?;
    let operation_id = mutation_operation_id(request)?;
    if let Some(id) = operation_id {
        validate_receipt_operation_id(id, receipt_metadata.kind)?;
        if let Some(prior) = stored_receipt(transaction, id)? {
            if prior.request_json != request_json
                || prior.kind != enum_text(&receipt_metadata.kind)?
                || prior.source_replica_id != receipt_metadata.source_replica_id
                || prior.wire_operation_id != receipt_metadata.wire_operation_id
            {
                return Err(DurableError::Refused(super::RefusalReason::OperationIdReused));
            }
            return Ok(Executed {
                response: serde_json::from_str(&prior.response_json).map_err(error)?,
                receipt: Some(ReceiptEvidence {
                    operation_id: id.clone(),
                    kind: receipt_metadata.kind,
                    source_replica_id: receipt_metadata.source_replica_id,
                    wire_operation_id: receipt_metadata.wire_operation_id,
                    sha256: prior.sha256,
                }),
                replayed: true,
            });
        }
    }
    let mut replica = load_replica(transaction, topology, replica_id)?;
    let before = replica.history.clone();
    let response = apply(configuration, &mut replica, request)?;
    for (id, fact) in &replica.history {
        if !before.contains_key(id) {
            let encoded = json(fact)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO facts (event_id, fact_json, sha256) VALUES (?, ?, ?)",
                    &[
                        Value::from(id.as_str()),
                        Value::from(&encoded),
                        Value::from(digest(&encoded)),
                    ],
                )
                .map_err(store_error)?;
            if inserted != 1 {
                return Err(DurableError::Storage(
                    "fact insert did not affect exactly one row".into(),
                ));
            }
        }
    }
    let receipt = if let Some(id) = operation_id {
        let response_json = json(&response)?;
        let sha256 = receipt_digest(
            topology.logical_manager_id(),
            id,
            &receipt_metadata,
            &request_json,
            &response_json,
        )?;
        let inserted = transaction
            .execute(
                "INSERT INTO receipts (operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256) VALUES (?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::from(id),
                    Value::from(enum_text(&receipt_metadata.kind)?),
                    receipt_metadata
                        .source_replica_id
                        .as_deref()
                        .map_or(Value::Null, Value::from),
                    receipt_metadata
                        .wire_operation_id
                        .as_deref()
                        .map_or(Value::Null, Value::from),
                    Value::from(&request_json),
                    Value::from(&response_json),
                    Value::from(sha256.as_str()),
                ],
            )
            .map_err(store_error)?;
        if inserted != 1 {
            return Err(DurableError::Storage(
                "receipt insert did not affect exactly one row".into(),
            ));
        }
        Some(ReceiptEvidence {
            operation_id: id.clone(),
            kind: receipt_metadata.kind,
            source_replica_id: receipt_metadata.source_replica_id,
            wire_operation_id: receipt_metadata.wire_operation_id,
            sha256,
        })
    } else {
        None
    };
    Ok(Executed {
        response,
        receipt,
        replayed: false,
    })
}

fn verify_stored_receipt(logical_manager_id: &str, receipt: &StoredReceipt) -> DurableResult<()> {
    let metadata = ReceiptMetadata {
        kind: enum_from_text(&receipt.kind)
            .map_err(|_| DurableError::Corrupt("stored receipt kind is invalid".into()))?,
        source_replica_id: receipt.source_replica_id.clone(),
        wire_operation_id: receipt.wire_operation_id.clone(),
    };
    validate_receipt_metadata(
        logical_manager_id,
        &receipt.operation_id,
        &receipt.request_json,
        &metadata,
    )?;
    if receipt_digest(
        logical_manager_id,
        &receipt.operation_id,
        &metadata,
        &receipt.request_json,
        &receipt.response_json,
    )? != receipt.sha256
    {
        return Err(DurableError::Corrupt("stored receipt hash mismatch".into()));
    }
    serde_json::from_str::<Response>(&receipt.response_json)
        .map_err(|_| DurableError::Corrupt("stored receipt response is invalid".into()))?;
    Ok(())
}

/// A manager journal opened from a store profile on MariaDB.
pub struct MariaDbJournal {
    store: Box<dyn store::DurableStore>,
    configuration: Configuration,
    topology: Topology,
    replica_id: String,
    closed_state: StoreClosedState,
}

impl MariaDbJournal {
    /// Opens through [`podmesh::open_manager_store`] and binds identity for this replica.
    ///
    /// # Errors
    /// Refuses invalid configuration, profile validation failures, and store faults.
    pub fn open(
        profile: &StoreConfig,
        configuration: Configuration,
        replica_id: &str,
    ) -> DurableResult<Self> {
        let topology = validate_local_configuration(&configuration, replica_id)?;
        profile
            .validate()
            .map_err(|problem| DurableError::Storage(problem.to_string()))?;
        let opened = podmesh::open_manager_store(profile)
            .map_err(|problem| DurableError::Storage(problem.to_string()))?;
        let mut store = match opened.into_journal() {
            ManagerJournal::Durable(store) => store,
            ManagerJournal::Sqlite(_) => {
                return Err(DurableError::Storage(
                    "manager store profile named sqlite; use ConfiguredStore on a file path instead"
                        .into(),
                ));
            }
        };
        ensure_identity(store.as_mut(), &topology, replica_id)?;
        Ok(Self {
            store,
            configuration,
            topology,
            replica_id: replica_id.into(),
            closed_state: store_closed_state_untracked(),
        })
    }

    /// Applies one laboratory request in a single durable transaction.
    ///
    /// # Errors
    /// Returns the same refusals as the file-backed [`Store`] for supported requests.
    pub fn execute(&mut self, request: &Request) -> DurableResult<Response> {
        Ok(self.execute_with_receipt(request)?.response)
    }

    /// Same contract as [`Store::execute_with_receipt`].
    ///
    /// # Errors
    /// Returns durable refusals from the active backend.
    pub fn execute_with_receipt(&mut self, request: &Request) -> DurableResult<Executed> {
        let receipt_metadata = receipt_metadata_for_request(request);
        let mut transaction = self.store.transaction().map_err(store_error)?;
        let executed = execute_on_transaction(
            transaction.as_mut(),
            &self.configuration,
            &self.topology,
            &self.replica_id,
            request,
            receipt_metadata,
        )?;
        transaction.commit().map_err(store_error)?;
        Ok(executed)
    }

    /// Same contract as [`Store::highest_local_observation`].
    ///
    /// # Errors
    /// Returns store faults and corrupt receipt rows.
    pub fn highest_local_observation(&mut self) -> DurableResult<Option<u64>> {
        let logical_manager_id = self.topology.logical_manager_id();
        let observe_kind = enum_text(&ReceiptKind::Observe)?;
        let mut transaction = self.store.transaction().map_err(store_error)?;
        let rows = transaction
            .query(
                "SELECT operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256 FROM receipts WHERE kind = ?",
                &[Value::from(&observe_kind)],
            )
            .map_err(store_error)?;
        let mut highest = None;
        for row in rows {
            let stored = read_stored_receipt(&row)?;
            verify_stored_receipt(logical_manager_id, &stored)?;
            let response = serde_json::from_str(&stored.response_json).map_err(error)?;
            if let Response::Observed { fact } = response {
                if fact.origin_replica_id == self.replica_id {
                    highest = highest.max(Some(fact.producer_sequence));
                }
            } else {
                return Err(DurableError::Corrupt(
                    "stored observe receipt does not record an observation".into(),
                ));
            }
        }
        transaction.commit().map_err(store_error)?;
        Ok(highest)
    }

    /// Process-local closed state (untracked for MariaDB in this slice).
    #[must_use]
    pub fn closed_state(&self) -> StoreClosedState {
        self.closed_state.clone()
    }

    /// Imports an authenticated peer snapshot with the same contract as [`Store`].
    ///
    /// # Errors
    /// Rejects invalid audit metadata, corrupt state, and failures before commit.
    pub fn execute_authenticated_import(
        &mut self,
        wire_operation_id: &str,
        snapshot: &Snapshot,
        mut audit: ExchangeAuditEvent,
    ) -> DurableResult<AuthenticatedImport> {
        if audit.direction != AuditDirection::Inbound
            || audit.phase != AuditPhase::InboundImportCommitted
            || audit.outcome != AuditOutcome::Accepted
            || audit.operation_id.as_deref() != Some(wire_operation_id)
            || audit.authenticated_peer_id.is_none()
            || audit.error_category.is_some()
            || audit.reason_code.is_some()
            || audit.local_receipt_operation_id.is_some()
            || audit.local_receipt_sha256.is_some()
            || audit.authenticated_peer_id.as_deref() != Some(snapshot.replica_id.as_str())
        {
            return Err(DurableError::InvalidAudit(
                "accepted inbound import fields are inconsistent".into(),
            ));
        }
        super::validate_token("wire operation ID", wire_operation_id)
            .map_err(|problem| DurableError::InvalidAudit(problem.to_string()))?;
        let local_operation_id = authenticated_import_receipt_id(
            &self.topology,
            &snapshot.replica_id,
            wire_operation_id,
        )
        .map_err(|problem| DurableError::InvalidAudit(problem.to_string()))?;
        let request = Request::Import {
            operation_id: local_operation_id.clone(),
            snapshot: snapshot.clone(),
        };
        let receipt_metadata = ReceiptMetadata {
            kind: ReceiptKind::AuthenticatedImport,
            source_replica_id: Some(snapshot.replica_id.clone()),
            wire_operation_id: Some(wire_operation_id.into()),
        };
        let mut transaction = self.store.transaction().map_err(store_error)?;
        journal_audit::prevalidate_authenticated_import_audit(
            transaction.as_mut(),
            &self.topology,
            &self.replica_id,
            &audit,
            &local_operation_id,
        )?;
        let executed = execute_on_transaction(
            transaction.as_mut(),
            &self.configuration,
            &self.topology,
            &self.replica_id,
            &request,
            receipt_metadata,
        )?;
        let receipt = executed.receipt.as_ref().ok_or_else(|| {
            DurableError::Corrupt("authenticated import has no receipt".into())
        })?;
        audit.local_receipt_operation_id = Some(receipt.operation_id.clone());
        audit.local_receipt_sha256 = Some(receipt.sha256.clone());
        audit.replayed = executed.replayed;
        let evidence = journal_audit::insert_audit(
            transaction.as_mut(),
            &self.topology,
            &self.replica_id,
            &audit,
            true,
        )?;
        transaction.commit().map_err(store_error)?;
        Ok(AuthenticatedImport {
            executed,
            audit: evidence,
        })
    }

    /// MariaDB journals do not run SQLite file integrity verification in this slice.
    ///
    /// # Errors
    /// Never fails in this slice.
    pub fn verify_full(&mut self) -> DurableResult<()> {
        Ok(())
    }

    /// Reports no SQLite file integrity state for this journal.
    ///
    /// # Errors
    /// Never fails in this slice.
    pub fn integrity(&self) -> DurableResult<StoreIntegrity> {
        Ok(StoreIntegrity {
            full_verifications: 0,
            last_full_verification_age: None,
            failure: None,
        })
    }
}

/// Manager journal opened either from a SQLite file path (default) or a store profile.
pub enum ConfiguredStore {
    Sqlite(Store),
    #[cfg(feature = "mariadb")]
    MariaDb(MariaDbJournal),
}

impl ConfiguredStore {
    /// Opens the journal: SQLite at `sqlite_database_path` unless `profile` names MariaDB.
    ///
    /// # Errors
    /// Refuses invalid configuration and store faults.
    pub fn open(
        profile: &StoreConfig,
        sqlite_database_path: &std::path::Path,
        configuration: Configuration,
        replica_id: &str,
    ) -> DurableResult<Self> {
        match profile.engine {
            podmesh::store::Engine::Sqlite => Ok(Self::Sqlite(
                Store::open(sqlite_database_path, configuration, replica_id)?,
            )),
            podmesh::store::Engine::Mariadb => {
                #[cfg(feature = "mariadb")]
                {
                    Ok(Self::MariaDb(MariaDbJournal::open(
                        profile,
                        configuration,
                        replica_id,
                    )?))
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

    /// Resolves `store.json` beside `profile_dir`, then opens.
    ///
    /// # Errors
    /// Refuses unreadable profiles and store faults.
    pub fn open_resolved(
        profile_dir: &std::path::Path,
        sqlite_database_path: &std::path::Path,
        configuration: Configuration,
        replica_id: &str,
    ) -> DurableResult<Self> {
        let profile = podmesh::resolve_manager_store_profile(profile_dir, sqlite_database_path)
            .map_err(|problem| DurableError::Storage(problem.to_string()))?;
        Self::open(&profile, sqlite_database_path, configuration, replica_id)
    }

    /// Applies one laboratory request.
    ///
    /// # Errors
    /// Returns durable refusals from the active backend.
    pub fn execute(&mut self, request: &Request) -> DurableResult<Response> {
        match self {
            Self::Sqlite(store) => store.execute(request),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.execute(request),
        }
    }

    /// Applies one request and returns durable receipt evidence for mutations.
    ///
    /// # Errors
    /// Returns durable refusals from the active backend.
    pub fn execute_with_receipt(&mut self, request: &Request) -> DurableResult<Executed> {
        match self {
            Self::Sqlite(store) => store.execute_with_receipt(request),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.execute_with_receipt(request),
        }
    }

    /// Highest local `observe` sequence, or `None` when this replica appended none.
    ///
    /// # Errors
    /// Returns store faults and corrupt receipt rows.
    pub fn highest_local_observation(&mut self) -> DurableResult<Option<u64>> {
        match self {
            Self::Sqlite(store) => store.highest_local_observation(),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.highest_local_observation(),
        }
    }

    /// The closed state of this journal in this process.
    #[must_use]
    pub fn closed_state(&self) -> StoreClosedState {
        match self {
            Self::Sqlite(store) => store.closed_state(),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.closed_state(),
        }
    }

    /// Repeats complete store verification where the backend supports it.
    ///
    /// # Errors
    /// Returns verification failures that close SQLite-backed journals.
    pub fn verify_full(&mut self) -> DurableResult<()> {
        match self {
            Self::Sqlite(store) => store.verify_full(),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.verify_full(),
        }
    }

    /// Observable integrity state of this journal in this process.
    ///
    /// # Errors
    /// Reports a poisoned integrity lock on SQLite-backed journals.
    pub fn integrity(&self) -> DurableResult<StoreIntegrity> {
        match self {
            Self::Sqlite(store) => store.integrity(),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.integrity(),
        }
    }

    /// Imports an authenticated peer snapshot on the active journal backend.
    ///
    /// # Errors
    /// Rejects invalid audit metadata and failures before commit.
    pub fn execute_authenticated_import(
        &mut self,
        wire_operation_id: &str,
        snapshot: &Snapshot,
        audit: ExchangeAuditEvent,
    ) -> DurableResult<AuthenticatedImport> {
        match self {
            Self::Sqlite(store) => {
                store.execute_authenticated_import(wire_operation_id, snapshot, audit)
            }
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => {
                store.execute_authenticated_import(wire_operation_id, snapshot, audit)
            }
        }
    }
}
