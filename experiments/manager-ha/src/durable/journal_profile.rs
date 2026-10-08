//! MariaDB manager journal entry path: open through `podmesh::open_manager_store` and serve the
//! laboratory request surface on [`DurableStore`]. SQLite keeps the file-backed [`Store`].

use std::sync::Arc;
use std::time::Instant;

use super::{
    apply, authenticated_import_receipt_id, closed_state, corrupt_model, decode_fact_row, digest,
    enum_from_text, error, inspect_profile, integrity_state, json, lock, mutation_operation_id,
    receipt_digest, receipt_metadata_for_request, validate_local_configuration,
    validate_receipt_metadata, validate_receipt_operation_id, AuditDirection, AuditOutcome,
    AuditPhase, AuthenticatedImport, Configuration, DurableError, DurableResult, Executed,
    ExchangeAuditEvent, ExchangeAuditEvidence, ReceiptEvidence, ReceiptKind, ReceiptMetadata, Request,
    Response, Snapshot, Store, StoreClosedState, StoreIntegrity, StoreIntegrityEntry, Topology,
    VerifiedPositions,
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
    integrity: Arc<StoreIntegrityEntry>,
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
        let integrity = Arc::new(StoreIntegrityEntry::default());
        let mut journal = Self {
            store,
            configuration,
            topology,
            replica_id: replica_id.into(),
            integrity,
        };
        journal.run_full_verification(true)?;
        Ok(journal)
    }

    fn run_full_verification(&mut self, replace_positions: bool) -> DurableResult<()> {
        let integrity = Arc::clone(&self.integrity);
        let _pass = lock(&integrity.full_pass, "full verification lock")?;
        integrity.refuse_if_failed()?;
        let started = Instant::now();
        let check = self.store.integrity_check().map_err(store_error)?;
        if !check.ok {
            return Err(integrity.fail_closed(DurableError::Corrupt(
                "MariaDB store integrity check failed".into(),
            )));
        }
        let mut transaction = self.store.transaction().map_err(store_error)?;
        inspect_profile::verify_mariadb_journal_rows(
            transaction.as_mut(),
            &self.topology,
            &self.replica_id,
        )
        .map_err(|problem| integrity.fail_closed_if_corrupt(problem))?;
        transaction.commit().map_err(store_error)?;
        integrity.record_full_verification(VerifiedPositions::default(), started, replace_positions)?;
        Ok(())
    }

    /// Allocates the same bounded, entropy-backed attempt identity as SQLite.
    ///
    /// # Errors
    /// Refuses invalid nonces and clock/entropy faults.
    pub fn new_attempt_id(&self, wire_nonce: &str) -> DurableResult<String> {
        super::allocate_attempt_id(&self.replica_id, wire_nonce)
    }

    /// Persists a non-import exchange phase on this private MariaDB journal.
    ///
    /// # Errors
    /// Refuses invalid audit/receipt bindings, corruption and failed commits.
    pub fn record_exchange_audit(
        &mut self,
        audit: &ExchangeAuditEvent,
    ) -> DurableResult<ExchangeAuditEvidence> {
        self.integrity.refuse_if_failed()?;
        if audit.phase == AuditPhase::InboundImportCommitted {
            return Err(DurableError::InvalidAudit(
                "inbound import audit must use the atomic authenticated-import API".into(),
            ));
        }
        let mut transaction = self.store.transaction().map_err(store_error)?;
        let evidence = journal_audit::insert_audit(
            transaction.as_mut(),
            &self.topology,
            &self.replica_id,
            audit,
            false,
        )
        .map_err(|problem| self.integrity.fail_closed_if_corrupt(problem))?;
        transaction.commit().map_err(store_error)?;
        Ok(evidence)
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
        self.integrity.refuse_if_failed()?;
        let receipt_metadata = receipt_metadata_for_request(request);
        let mut transaction = self.store.transaction().map_err(store_error)?;
        let executed = execute_on_transaction(
            transaction.as_mut(),
            &self.configuration,
            &self.topology,
            &self.replica_id,
            request,
            receipt_metadata,
        )
        .map_err(|problem| self.integrity.fail_closed_if_corrupt(problem))?;
        transaction.commit().map_err(store_error)?;
        Ok(executed)
    }

    /// Same contract as [`Store::highest_local_observation`].
    ///
    /// # Errors
    /// Returns store faults and corrupt receipt rows.
    pub fn highest_local_observation(&mut self) -> DurableResult<Option<u64>> {
        self.integrity.refuse_if_failed()?;
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
            verify_stored_receipt(logical_manager_id, &stored)
                .map_err(|problem| self.integrity.fail_closed_if_corrupt(problem))?;
            let response = serde_json::from_str(&stored.response_json).map_err(error)?;
            if let Response::Observed { fact } = response {
                if fact.origin_replica_id == self.replica_id {
                    highest = highest.max(Some(fact.producer_sequence));
                }
            } else {
                return Err(self.integrity.fail_closed_if_corrupt(DurableError::Corrupt(
                    "stored observe receipt does not record an observation".into(),
                )));
            }
        }
        transaction.commit().map_err(store_error)?;
        Ok(highest)
    }

    /// Process-local closed state for this journal in this process.
    #[must_use]
    pub fn closed_state(&self) -> StoreClosedState {
        StoreClosedState(Arc::clone(&self.integrity))
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
        self.integrity.refuse_if_failed()?;
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

    /// Verifies CHECK TABLE on the MariaDB journal and every stored fact, receipt
    /// and audit row in one read transaction.
    ///
    /// # Errors
    /// Returns verification failures that close this journal for the rest of the
    /// process, or a storage error that prevented the pass.
    pub fn verify_full(&mut self) -> DurableResult<()> {
        self.run_full_verification(false)
    }

    /// Reports the integrity state of this journal in this process.
    ///
    /// # Errors
    /// Reports a poisoned integrity lock.
    pub fn integrity(&self) -> DurableResult<StoreIntegrity> {
        let failure = closed_state(&self.integrity)?.clone();
        let state = integrity_state(&self.integrity)?;
        Ok(StoreIntegrity {
            full_verifications: state.full_verifications,
            last_full_verification_age: state.last_full_verification.map(|at| at.elapsed()),
            failure,
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

    /// Allocates an exchange attempt on the selected backend without opening another store.
    ///
    /// # Errors
    /// Refuses invalid nonces and clock/entropy faults.
    pub fn new_attempt_id(&self, wire_nonce: &str) -> DurableResult<String> {
        match self {
            Self::Sqlite(store) => store.new_attempt_id(wire_nonce),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.new_attempt_id(wire_nonce),
        }
    }

    /// Records an exchange phase in the same journal as facts and receipts.
    ///
    /// # Errors
    /// Refuses invalid audit evidence and store faults.
    pub fn record_exchange_audit(
        &mut self,
        audit: &ExchangeAuditEvent,
    ) -> DurableResult<ExchangeAuditEvidence> {
        match self {
            Self::Sqlite(store) => store.record_exchange_audit(audit),
            #[cfg(feature = "mariadb")]
            Self::MariaDb(store) => store.record_exchange_audit(audit),
        }
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
