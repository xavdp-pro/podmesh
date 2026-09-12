//! Local SQLite persistence and a typed process boundary for the laboratory.
//!
//! Supplied peer snapshots are untrusted observations, never remote authority.

use std::{path::Path, time::Duration};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Conflict, Fact, Reconciliation, Replica, ReplicaConfig, Result, ScopeGrant, Topology};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub logical_manager_id: String,
    pub replicas: Vec<ReplicaConfig>,
    pub grants: Vec<ScopeGrant>,
}

impl Configuration {
    /// Validates the same static topology used by the deterministic model.
    ///
    /// # Errors
    /// Returns the model's identity or scope validation error.
    pub fn topology(&self) -> Result<Topology> {
        Topology::new(
            &self.logical_manager_id,
            self.replicas.clone(),
            self.grants.clone(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub configuration: Configuration,
    pub replica_id: String,
    pub facts: Vec<Fact>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Observe {
        operation_id: String,
        scope: String,
        subject: String,
        exclusive_resource: Option<String>,
        active_claim: bool,
        value: String,
    },
    Import {
        operation_id: String,
        snapshot: Snapshot,
    },
    Export {},
    Inspect {},
    CheckService {
        snapshots: Vec<Snapshot>,
        exclusive_service: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Observed {
        fact: Fact,
    },
    Imported {
        inserted: usize,
        history_len: usize,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Inspection {
        history_len: usize,
        current: Vec<Fact>,
        conflicts: Vec<Conflict>,
        blocked_exclusive_resources: Vec<String>,
    },
    ServiceCheck {
        coordinator_replica_id: String,
        eligible_in_supplied_history: bool,
    },
}

/// A local store. Every request reloads immutable history inside a transaction.
pub struct Store {
    connection: Connection,
    configuration: Configuration,
    topology: Topology,
    replica_id: String,
}

impl Store {
    /// Opens or initializes a schema-v2 database bound to one replica/topology.
    /// Uses SQLite WAL with synchronous FULL; no database file is exchanged.
    ///
    /// # Errors
    /// Rejects invalid configuration, mismatched identity, unknown schema and I/O errors.
    pub fn open(path: &Path, configuration: Configuration, replica_id: &str) -> Result<Self> {
        let topology = configuration.topology()?;
        topology.instantiate(replica_id)?;
        let mut connection = Connection::open(path).map_err(error)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let version: u32 = transaction
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(error)?;
        if version == 0 {
            let tables: u32 = transaction
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                    [],
                    |row| row.get(0),
                )
                .map_err(error)?;
            if tables != 0 {
                return Err("refusing an unrecognized database".into());
            }
            transaction.execute_batch(SCHEMA).map_err(error)?;
            transaction
                .execute(
                    "INSERT INTO identity VALUES (1, ?1, ?2)",
                    params![replica_id, json(&topology)?],
                )
                .map_err(error)?;
        } else if version != 2 {
            return Err("unsupported manager laboratory schema".into());
        }
        let identity: (String, String) = transaction
            .query_row(
                "SELECT replica_id, topology_json FROM identity WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(error)?;
        if identity != (replica_id.to_string(), json(&topology)?) {
            return Err(
                "database identity or topology mismatch; copied state grants no identity".into(),
            );
        }
        transaction.commit().map_err(error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(error)?;
        Ok(Self {
            connection,
            configuration,
            topology,
            replica_id: replica_id.into(),
        })
    }

    /// Applies one request atomically, acknowledging mutations only after commit.
    /// Mutation operation IDs replay the original result; incompatible reuse fails.
    ///
    /// # Errors
    /// Rejects corrupt history, invalid facts, divergent reconciliation, conflicting
    /// retries, and SQLite failures. No partial import is committed.
    pub fn execute(&mut self, request: &Request) -> Result<Response> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        verify_receipts(&transaction)?;
        let mut replica = load(&transaction, &self.topology, &self.replica_id)?;
        let before = replica.history.clone();
        let request_json = json(request)?;
        let operation_id = match request {
            Request::Observe { operation_id, .. } | Request::Import { operation_id, .. } => {
                if operation_id.is_empty() {
                    return Err("operation ID is required".into());
                }
                Some(operation_id)
            }
            _ => None,
        };
        if let Some(id) = operation_id {
            let prior: Option<(String, String)> = transaction
                .query_row(
                    "SELECT request_json, response_json FROM receipts WHERE operation_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(error)?;
            if let Some((original, response)) = prior {
                if original != request_json {
                    return Err("operation ID reused with different request".into());
                }
                return serde_json::from_str(&response).map_err(error);
            }
        }
        let response = apply(&self.configuration, &mut replica, request)?;
        for (id, fact) in &replica.history {
            if !before.contains_key(id) {
                let encoded = json(fact)?;
                transaction
                    .execute(
                        "INSERT INTO facts(event_id, fact_json, sha256) VALUES (?1, ?2, ?3)",
                        params![id, encoded, digest(&encoded)],
                    )
                    .map_err(error)?;
            }
        }
        if let Some(id) = operation_id {
            let response_json = json(&response)?;
            let checksum = receipt_digest(id, &request_json, &response_json)?;
            transaction
                .execute(
                    "INSERT INTO receipts (operation_id, request_json, response_json, sha256) VALUES (?1, ?2, ?3, ?4)",
                    params![id, request_json, response_json, checksum],
                )
                .map_err(error)?;
        }
        transaction.commit().map_err(error)?;
        Ok(response)
    }
}

fn apply(
    configuration: &Configuration,
    replica: &mut Replica,
    request: &Request,
) -> Result<Response> {
    match request {
        Request::Observe {
            scope,
            subject,
            exclusive_resource,
            active_claim,
            value,
            ..
        } => Ok(Response::Observed {
            fact: replica.observe(
                scope,
                subject,
                exclusive_resource.as_deref(),
                *active_claim,
                value,
            )?,
        }),
        Request::Import { snapshot, .. } => {
            let peer = restore_snapshot(&replica.topology, snapshot)?;
            let before = replica.history_len();
            for fact in peer.history.into_values() {
                replica.ingest(fact)?;
            }
            Ok(Response::Imported {
                inserted: replica.history_len() - before,
                history_len: replica.history_len(),
            })
        }
        Request::Export {} => Ok(Response::Snapshot {
            snapshot: Snapshot {
                configuration: configuration.clone(),
                replica_id: replica.replica_id().into(),
                facts: replica.history.values().cloned().collect(),
            },
        }),
        Request::Inspect {} => {
            let view = replica.materialize();
            Ok(Response::Inspection {
                history_len: replica.history_len(),
                current: view.current.into_values().collect(),
                conflicts: view.conflicts,
                blocked_exclusive_resources: view.blocked_exclusive_resources.into_iter().collect(),
            })
        }
        Request::CheckService {
            snapshots,
            exclusive_service,
        } => {
            let peers: Vec<_> = snapshots
                .iter()
                .map(|snapshot| restore_snapshot(&replica.topology, snapshot))
                .collect::<Result<_>>()?;
            // Include current local state, never substitute an old caller-supplied local copy.
            let mut copies = vec![&*replica];
            copies.extend(peers.iter());
            let reconciled = Reconciliation::after_full_exchange(&replica.topology, &copies)?;
            let permit = reconciled
                .authorize_exclusive_service(replica.replica_id(), exclusive_service)
                .ok();
            Ok(Response::ServiceCheck {
                coordinator_replica_id: reconciled.coordinator_replica_id().into(),
                eligible_in_supplied_history: replica
                    .advertises(exclusive_service, permit.as_ref()),
            })
        }
    }
}

fn restore_snapshot(topology: &Topology, snapshot: &Snapshot) -> Result<Replica> {
    if snapshot.configuration.topology()? != *topology {
        return Err("snapshot topology mismatch".into());
    }
    let mut replica = topology.instantiate(&snapshot.replica_id)?;
    for fact in &snapshot.facts {
        replica.ingest(fact.clone())?;
    }
    Ok(replica)
}

fn load(connection: &Connection, topology: &Topology, id: &str) -> Result<Replica> {
    let mut replica = topology.instantiate(id)?;
    let mut statement = connection
        .prepare("SELECT event_id, fact_json, sha256 FROM facts ORDER BY event_id")
        .map_err(error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(error)?;
    for row in rows {
        let (id, encoded, hash) = row.map_err(error)?;
        if digest(&encoded) != hash {
            return Err("stored fact hash mismatch".into());
        }
        let fact: Fact = serde_json::from_str(&encoded).map_err(error)?;
        if fact.event_id != id {
            return Err("stored event identity mismatch".into());
        }
        replica.ingest(fact)?;
    }
    Ok(replica)
}

fn json(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(error)
}
fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

// A domain-separated JSON tuple is unambiguous even when fields contain quotes,
// delimiters, newlines or NULs. The checksum binds exact stored JSON strings,
// including whitespace; it is an integrity check, not origin authentication.
fn receipt_digest(operation_id: &str, request_json: &str, response_json: &str) -> Result<String> {
    Ok(digest(&json(&(
        "podmesh-manager-ha-receipt/1",
        operation_id,
        request_json,
        response_json,
    ))?))
}

fn verify_receipts(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT operation_id, request_json, response_json, sha256 FROM receipts ORDER BY operation_id",
    ).map_err(error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(error)?;
    for row in rows {
        let (id, request, response, checksum) = row.map_err(error)?;
        if receipt_digest(&id, &request, &response)? != checksum {
            return Err("stored receipt hash mismatch".into());
        }
    }
    Ok(())
}

const SCHEMA: &str = "
CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), replica_id TEXT NOT NULL, topology_json TEXT NOT NULL);
CREATE TABLE facts (event_id TEXT PRIMARY KEY, fact_json TEXT NOT NULL, sha256 TEXT NOT NULL);
CREATE TABLE receipts (operation_id TEXT PRIMARY KEY, request_json TEXT NOT NULL, response_json TEXT NOT NULL, sha256 TEXT NOT NULL);
CREATE TRIGGER facts_no_update BEFORE UPDATE ON facts BEGIN SELECT RAISE(ABORT, 'immutable fact'); END;
CREATE TRIGGER facts_no_delete BEFORE DELETE ON facts BEGIN SELECT RAISE(ABORT, 'immutable fact'); END;
CREATE TRIGGER identity_no_update BEFORE UPDATE ON identity BEGIN SELECT RAISE(ABORT, 'immutable identity'); END;
CREATE TRIGGER identity_no_delete BEFORE DELETE ON identity BEGIN SELECT RAISE(ABORT, 'immutable identity'); END;
CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;
CREATE TRIGGER receipts_no_delete BEFORE DELETE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;
PRAGMA user_version=2;
";
