//! MariaDB manager journal entry path: open through `podmesh::open_manager_store` and serve the
//! laboratory request surface on [`DurableStore`]. SQLite keeps the file-backed [`Store`].

use super::{
    apply, corrupt_model, decode_fact_row, digest, error, json, mutation_operation_id,
    receipt_digest, receipt_metadata_for_request, validate_local_configuration,
    validate_receipt_operation_id, Configuration, DurableError, DurableResult, Executed,
    ReceiptEvidence, ReceiptMetadata, Request, Response, Store, Topology,
};
use podmesh::store::{self, Row, StoreConfig, Transaction, Value};
use podmesh::ManagerJournal;

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
            .ingest(decode_fact_row(&id, &encoded, &hash)?)
            .map_err(corrupt_model)?;
    }
    Ok(replica)
}

struct StoredReceipt {
    request_json: String,
    kind: String,
    source_replica_id: Option<String>,
    wire_operation_id: Option<String>,
    response_json: String,
    sha256: String,
}

fn read_stored_receipt(row: &Row) -> DurableResult<StoredReceipt> {
    Ok(StoredReceipt {
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
    Ok(rows.first().map(read_stored_receipt).transpose()?)
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
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                    receipt_metadata
                        .wire_operation_id
                        .as_deref()
                        .map(Value::from)
                        .unwrap_or(Value::Null),
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

/// A manager journal opened from a store profile on MariaDB.
pub struct MariaDbJournal {
    store: Box<dyn store::DurableStore>,
    configuration: Configuration,
    topology: Topology,
    replica_id: String,
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
        })
    }

    /// Applies one laboratory request in a single durable transaction.
    ///
    /// # Errors
    /// Returns the same refusals as the file-backed [`Store`] for supported requests.
    pub fn execute(&mut self, request: &Request) -> DurableResult<Response> {
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
        Ok(executed.response)
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
}
