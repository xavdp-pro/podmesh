//! Local SQLite persistence and a typed process boundary for the laboratory.
//!
//! Supplied peer snapshots are untrusted observations, never remote authority.

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("the Stage D store capture ABI is qualified only for Linux x86_64");

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Conflict, Fact, Reconciliation, Replica, ReplicaConfig, ScopeGrant, Topology};

type ModelResult<T> = crate::Result<T>;
pub type DurableResult<T> = std::result::Result<T, DurableError>;
type PreflightKey = (u64, u64, u64, i64, i64, i64, i64, String, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    InvalidRequest,
    PolicyViolation,
    OperationIdReused,
    UnsupportedSchema,
    IdentityMismatch,
    UnrecognizedStore,
    MissingStore,
    UnsafeStore,
    TransportUnavailable,
}

impl std::fmt::Display for RefusalReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::InvalidRequest => "invalid_request",
            Self::PolicyViolation => "policy_violation",
            Self::OperationIdReused => "operation_id_reused",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::IdentityMismatch => "identity_mismatch",
            Self::UnrecognizedStore => "unrecognized_store",
            Self::MissingStore => "missing_store",
            Self::UnsafeStore => "unsafe_store",
            Self::TransportUnavailable => "transport_unavailable",
        };
        formatter.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", content = "detail", rename_all = "snake_case")]
pub enum DurableError {
    Refused(RefusalReason),
    Corrupt(String),
    Storage(String),
    InvalidAudit(String),
}

impl std::fmt::Display for DurableError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => write!(formatter, "refused: {reason}"),
            Self::Corrupt(detail) => write!(formatter, "corrupt: {detail}"),
            Self::Storage(detail) => write!(formatter, "storage: {detail}"),
            Self::InvalidAudit(detail) => write!(formatter, "invalid_audit: {detail}"),
        }
    }
}

impl std::error::Error for DurableError {}

impl From<DurableError> for String {
    fn from(value: DurableError) -> Self {
        value.to_string()
    }
}

static NEXT_INSPECTION_SNAPSHOT: AtomicU64 = AtomicU64::new(1);
static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);
static EXPECTED_SCHEMA_SHAPE: OnceLock<Vec<(String, String, String)>> = OnceLock::new();
static PREFLIGHTED_STORES: OnceLock<Mutex<BTreeSet<PreflightKey>>> = OnceLock::new();
static VERIFIED_STORES: OnceLock<Mutex<BTreeMap<IntegrityKey, Arc<StoreIntegrityEntry>>>> =
    OnceLock::new();
static LIVE_FILES: OnceLock<Mutex<BTreeMap<LiveFileKey, Arc<LiveFile>>>> = OnceLock::new();

/// Default interval between two complete background verifications of one
/// store by a long-running process. The complete verification also runs once
/// when a process first opens a store.
pub const DEFAULT_FULL_VERIFICATION_INTERVAL: Duration = Duration::from_secs(600);

/// Process-local identity of one database file: device, inode, birth time when
/// the filesystem reports it, replica and topology. Birth time distinguishes a
/// new file that reuses the inode of a deleted one.
type IntegrityKey = (u64, u64, Option<(u64, u32)>, String, String);

/// The identity of one database file: device, inode and birth time.
type FileIdentity = (u64, u64, Option<(u64, u32)>);

/// One database path of this process, by directory entry: the device and inode
/// of its directory, and its file name. Two spellings of a path name one entry.
type LiveFileKey = (u64, u64, OsString);

/// The SQLite connections this module holds on one database path, and the
/// database file they have open.
///
/// POSIX advisory locks belong to a process and an inode, and closing any
/// descriptor of a file releases every lock the process holds on that file ("How
/// To Corrupt An SQLite Database File", section 2.2). SQLite keeps the descriptors
/// of its own connections open until the last of them on a file closes, but a
/// descriptor opened around SQLite, then closed, releases the locks of every
/// connection the process has on the file. Another process could then take the
/// write lock, check the WAL into the database under a snapshot a connection still
/// reads, or delete the WAL at its close as the last connection while this process
/// keeps committing into it. So this process opens the database, WAL and SHM files
/// itself, to copy them for a preflight or an inspection, only while it holds no
/// connection on the path, and holds `gate` while it copies them, so that no
/// connection opens meanwhile.
#[derive(Default)]
struct LiveFile {
    /// Connections open or opening on the path. A connection is counted, under
    /// `gate`, before SQLite opens the file, and uncounted once SQLite has closed it.
    connections: AtomicUsize,
    /// Held while the files are copied and while a connection opens; holds the
    /// identity of the file that the counted connections have open.
    gate: Mutex<Option<FileIdentity>>,
}

/// One connection counted on a database path. It is declared after the
/// connection it counts, so that it is dropped once SQLite has closed that
/// connection.
struct LiveConnection(Arc<LiveFile>);

impl LiveConnection {
    fn count(file: &Arc<LiveFile>) -> Self {
        file.connections.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(file))
    }
}

impl Drop for LiveConnection {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The integrity model of one database file within one process.
///
/// A complete verification of every fact, receipt and audit row runs when the
/// process first opens the file and whenever [`Store::verify_full`] runs (the
/// periodic pass). Every transaction verifies the schema shape, checks that the
/// last verified row of each table is unchanged and that no row precedes the
/// first, and verifies the rows appended after the last verified row, which must
/// continue the table's rowids without a gap (see [`verify_contiguous_rowids`]).
/// Rows the operation reads or extends (a replayed receipt, a replayed audit
/// event, the audit rows of the attempt it appends to, the facts it loads) are
/// verified when read. Any failure to read or verify a stored row once the
/// transaction holds its snapshot, and a schema found corrupt when the file is
/// opened, fails the file closed for the rest of the process: every later open,
/// write and export of that file returns that same error, and no transaction of
/// the process commits on it afterwards. Another file at the same path is another
/// database file. An in-place change to an older row that no operation reads,
/// including a replaced row or a removed row that leaves a gap, is detected by the
/// next complete verification, not by the next transaction; a table whose last
/// rows were removed is shorter and still verifies when no remaining row depends
/// on them.
#[derive(Default)]
struct StoreIntegrityEntry {
    state: Mutex<IntegrityState>,
    full_pass: Mutex<()>,
}

#[derive(Default)]
struct IntegrityState {
    positions: Option<VerifiedPositions>,
    failure: Option<DurableError>,
    last_full_verification: Option<Instant>,
    full_verifications: u64,
}

/// The last verified row of each append-only table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct VerifiedPositions {
    facts: TablePosition,
    receipts: TablePosition,
    audits: TablePosition,
}

/// A verified table prefix: the table's rows were exactly the rowids 1 to
/// `rowid` (none when `rowid` is 0) and every one of them was verified; `sha256`
/// is the stored checksum of the row at `rowid`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TablePosition {
    rowid: i64,
    sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppendOnlyTable {
    Facts,
    Receipts,
    Audits,
}

impl AppendOnlyTable {
    const ALL: [Self; 3] = [Self::Facts, Self::Receipts, Self::Audits];

    const fn name(self) -> &'static str {
        match self {
            Self::Facts => "facts",
            Self::Receipts => "receipts",
            Self::Audits => "exchange_audit_events",
        }
    }
}

/// Observable integrity state of one store in this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreIntegrity {
    /// Complete verifications that succeeded in this process, including the one
    /// run when the process first opened the store.
    pub full_verifications: u64,
    /// Time since the last successful complete verification started.
    pub last_full_verification_age: Option<Duration>,
    /// The verification failure that closed this store for the process, if any.
    pub failure: Option<DurableError>,
}

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
    pub fn topology(&self) -> ModelResult<Topology> {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptEvidence {
    /// Local, bounded durable key. It is never a peer authority token.
    pub operation_id: String,
    pub kind: ReceiptKind,
    pub source_replica_id: Option<String>,
    pub wire_operation_id: Option<String>,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    Observe,
    LaboratoryImport,
    AuthenticatedImport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executed {
    pub response: Response,
    pub receipt: Option<ReceiptEvidence>,
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDirection {
    Inbound,
    Outbound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPhase {
    OutboundRequestPrepared,
    OutboundExchangeCompleted,
    InboundRequestObserved,
    InboundImportCommitted,
    InboundRefusalRecorded,
    InboundReplyPrepared,
    InboundReplyWriteObserved,
    InboundDiagnosticReplyWritten,
    InboundConnectionClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Accepted,
    AuthenticatedRefusal,
    UnauthenticatedDiagnostic,
    Unavailable,
    Malformed,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditErrorCategory {
    Unavailable,
    Refused,
    Malformed,
}

/// A bounded typed event supplied by the network layer after it has classified
/// one exchange phase. Durable receipt fields are filled by the store for an
/// accepted inbound import and may otherwise reference an existing receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeAuditEvent {
    pub audit_event_id: String,
    /// A locally generated identity for this exchange attempt.
    pub attempt_id: String,
    /// A validated decoded peer protocol nonce, or a locally generated
    /// `preauth:<64 lowercase hexadecimal SHA-256>` connection nonce before
    /// decoding reaches one. A pre-authentication nonce remains stable within
    /// that attempt and carries no peer, operation, receipt or replay authority.
    pub wire_nonce: String,
    pub direction: AuditDirection,
    pub phase: AuditPhase,
    pub authenticated_peer_id: Option<String>,
    pub peer_claim: Option<String>,
    pub operation_id: Option<String>,
    pub request_frame_bytes: u64,
    pub request_announced_body_bytes: Option<u64>,
    pub request_sha256: Option<String>,
    pub reply_frame_bytes: u64,
    pub reply_announced_body_bytes: Option<u64>,
    pub reply_sha256: Option<String>,
    pub outcome: AuditOutcome,
    pub error_category: Option<AuditErrorCategory>,
    pub reason_code: Option<RefusalReason>,
    pub local_receipt_operation_id: Option<String>,
    pub local_receipt_sha256: Option<String>,
    pub remote_receipt_operation_id: Option<String>,
    pub remote_receipt_sha256: Option<String>,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeAuditEvidence {
    pub event: ExchangeAuditEvent,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedImport {
    pub executed: Executed,
    pub audit: ExchangeAuditEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalStoreInspection {
    pub schema_version: u32,
    pub logical_manager_id: String,
    pub replica_id: String,
    pub history_count: usize,
    pub ordered_facts: Vec<Fact>,
    pub logical_history_sha256: String,
    pub current: Vec<Fact>,
    pub conflicts: Vec<Conflict>,
    pub blocked_exclusive_resources: Vec<String>,
    pub receipt_count: usize,
    pub ordered_receipts: Vec<ReceiptEvidence>,
    pub receipt_set_sha256: String,
    pub audit_event_count: usize,
    pub ordered_audit_events: Vec<ExchangeAuditEvidence>,
    pub audit_set_sha256: String,
    pub incomplete_attempts: Vec<IncompleteAttempt>,
    pub unaudited_import_receipt_ids: Vec<String>,
    pub sqlite_integrity_result: String,
}

/// The facts of a store as [`inspect_facts_read_only`] verifies them: the same
/// values as the fields of [`CanonicalStoreInspection`] with these names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactsInspection {
    pub history_count: usize,
    pub ordered_facts: Vec<Fact>,
    pub logical_history_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncompleteAttempt {
    pub direction: AuditDirection,
    pub attempt_id: String,
    pub wire_nonce: String,
    pub wire_operation_id: Option<String>,
    pub last_phase: AuditPhase,
}

/// A local store. Every request reloads immutable history inside a transaction.
/// Integrity follows the process-local model described on `StoreIntegrityEntry`.
pub struct Store {
    connection: Connection,
    /// Declared after `connection`, which is therefore closed before it is uncounted.
    _live: LiveConnection,
    configuration: Configuration,
    topology: Topology,
    replica_id: String,
    integrity: Arc<StoreIntegrityEntry>,
}

impl Store {
    /// Allocates a bounded local attempt identity. The wire nonce remains a
    /// separate audit field and may be reused by an untrusted peer. Before a
    /// peer nonce has been decoded, callers may supply only a locally generated
    /// `preauth:<64 lowercase hexadecimal SHA-256>` connection nonce.
    ///
    /// # Errors
    /// Refuses an invalid nonce and reports a local clock or serialization failure.
    pub fn new_attempt_id(&self, wire_nonce: &str) -> DurableResult<String> {
        validate_wire_nonce_value(wire_nonce)?;
        let sequence = NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(error)?
            .as_nanos();
        Ok(format!(
            "attempt:{}",
            digest(&json(&(
                "podmesh-manager-ha-attempt/1",
                &self.replica_id,
                wire_nonce,
                std::process::id(),
                sequence,
                timestamp,
                random_hex_16()?,
            ))?)
        ))
    }

    /// Opens or initializes a schema-v3 database bound to one replica/topology.
    /// Uses SQLite WAL with synchronous FULL; no database file is exchanged.
    ///
    /// The first open of a database file by a process verifies every stored
    /// fact, receipt and audit row in a read transaction. Later opens in the same
    /// process check only that the last verified row of each table is unchanged.
    ///
    /// An existing file is copied for a read-only preflight of its schema and
    /// identity only while this process holds no connection on the path (see
    /// `LiveFile`), and only when the process has not preflighted that file state
    /// before. While the process has the store open, an open requires the path to
    /// name the file that is open, and relies on the checks of its own transaction.
    ///
    /// # Errors
    /// Rejects invalid configuration, mismatched identity, unknown schema, I/O
    /// errors, another file at the path of a store this process has open, and a
    /// database file whose verification failed in this process, including a schema
    /// found corrupt by this open's preflight or transaction.
    pub fn open(
        path: &Path,
        configuration: Configuration,
        replica_id: &str,
    ) -> DurableResult<Self> {
        let topology = validate_local_configuration(&configuration, replica_id)?;
        let opened = open_counted_connection(path, &topology, replica_id)?;
        let OpenedFile {
            mut connection,
            counted,
            integrity,
            first_connection,
        } = opened;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(error)?;
        integrity.refuse_if_failed()?;
        // Read before the transaction takes its snapshot: every row named by these
        // positions was committed before that snapshot.
        let (positions, full_verifications) = integrity.positions()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        initialize_or_check_identity(&transaction, &topology, replica_id)
            .map_err(|problem| integrity.fail_closed_if_corrupt(problem))?;
        // A verified row that is missing or different in this snapshot was changed
        // in place, and the store is then verified completely again.
        let unchanged = match &positions {
            Some(positions) => positions_unchanged(&transaction, positions)
                .map_err(|problem| integrity.fail_closed(problem))?,
            None => false,
        };
        integrity.commit(transaction)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(error)?;
        if first_connection {
            let metadata = fs::symlink_metadata(path).map_err(error)?;
            remember_preflighted(preflight_key(&metadata, &topology, replica_id)?)?;
        }
        let mut store = Self {
            connection,
            _live: counted,
            configuration,
            topology,
            replica_id: replica_id.into(),
            integrity,
        };
        if !unchanged {
            store.verify_full_at_open(full_verifications)?;
        }
        Ok(store)
    }

    /// Verifies every stored fact, receipt and audit row in one read transaction,
    /// which does not block writers. This is the periodic pass of the integrity
    /// model; a long-running process runs it at an interval such as
    /// [`DEFAULT_FULL_VERIFICATION_INTERVAL`].
    ///
    /// # Errors
    /// Returns the verification failure, after which this store fails closed for
    /// the rest of the process, or a storage error that prevented the pass from
    /// taking its snapshot, which leaves the store open for a later pass.
    pub fn verify_full(&mut self) -> DurableResult<()> {
        let integrity = Arc::clone(&self.integrity);
        let _pass = lock(&integrity.full_pass, "full verification lock")?;
        integrity.refuse_if_failed()?;
        run_full_verification(
            &mut self.connection,
            &integrity,
            &self.topology,
            &self.replica_id,
            false,
        )
    }

    /// Runs [`Store::verify_full`] only when no successful complete verification
    /// of this store started in this process within `interval`.
    ///
    /// # Errors
    /// Returns the same errors as [`Store::verify_full`].
    pub fn verify_full_if_due(&mut self, interval: Duration) -> DurableResult<bool> {
        self.integrity.refuse_if_failed()?;
        let due = integrity_state(&self.integrity)?
            .last_full_verification
            .map_or(true, |started| started.elapsed() >= interval);
        if due {
            self.verify_full()?;
        }
        Ok(due)
    }

    /// Reports the integrity state of this store in this process.
    ///
    /// # Errors
    /// Reports a poisoned integrity lock.
    pub fn integrity(&self) -> DurableResult<StoreIntegrity> {
        let state = integrity_state(&self.integrity)?;
        Ok(StoreIntegrity {
            full_verifications: state.full_verifications,
            last_full_verification_age: state.last_full_verification.map(|at| at.elapsed()),
            failure: state.failure.clone(),
        })
    }

    fn verify_full_at_open(&mut self, full_verifications_seen: u64) -> DurableResult<()> {
        let integrity = Arc::clone(&self.integrity);
        let _pass = lock(&integrity.full_pass, "full verification lock")?;
        integrity.refuse_if_failed()?;
        // A complete verification that finished while this open waited replaced
        // the positions it found missing or changed; later transactions recheck
        // them before trusting them.
        if integrity.positions()?.1 > full_verifications_seen {
            return Ok(());
        }
        run_full_verification(
            &mut self.connection,
            &integrity,
            &self.topology,
            &self.replica_id,
            true,
        )
    }

    /// Runs one store operation and fails the store closed for the process when
    /// the operation found corrupt stored state.
    fn guarded<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> DurableResult<T>,
    ) -> DurableResult<T> {
        self.integrity.refuse_if_failed()?;
        let integrity = Arc::clone(&self.integrity);
        operation(self).map_err(|problem| integrity.fail_closed_if_corrupt(problem))
    }

    /// Applies one request atomically, acknowledging mutations only after commit.
    /// Mutation operation IDs replay the original result; incompatible reuse fails.
    ///
    /// # Errors
    /// Rejects corrupt history, invalid facts, divergent reconciliation, conflicting
    /// retries, and SQLite failures. No partial import is committed.
    pub fn execute(&mut self, request: &Request) -> DurableResult<Response> {
        Ok(self.execute_with_receipt(request)?.response)
    }

    /// Applies one request and returns durable receipt evidence for mutations.
    /// An identical retry returns the original response and receipt.
    /// Mutations run in a write transaction; `export`, `inspect` and
    /// `check_service` run in a read transaction that does not block writers.
    ///
    /// # Errors
    /// Rejects corrupt state, incompatible operation-ID reuse and failed commits.
    pub fn execute_with_receipt(&mut self, request: &Request) -> DurableResult<Executed> {
        let writes = matches!(request, Request::Observe { .. } | Request::Import { .. });
        let behavior = if writes {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        };
        let operation = |store: &mut Self| {
            let transaction = begin_verified(
                &mut store.connection,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                behavior,
            )?;
            let executed = execute_transaction(
                &transaction,
                &store.integrity,
                &store.configuration,
                &store.topology,
                &store.replica_id,
                request,
                receipt_metadata_for_request(request),
            )?;
            store.integrity.commit(transaction)?;
            Ok(executed)
        };
        self.guarded(operation)
    }

    /// Imports an authenticated peer snapshot and atomically commits its facts,
    /// mutation receipt and accepted inbound-import audit event.
    ///
    /// # Errors
    /// Rejects non-import requests, invalid audit metadata, corrupt state and any
    /// failure before the complete transaction commits.
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
        validate_token("wire operation ID", wire_operation_id)
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
        self.guarded(|store| {
            let transaction = begin_verified(
                &mut store.connection,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                TransactionBehavior::Immediate,
            )?;
            prevalidate_authenticated_import_audit(
                &transaction,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                &audit,
                &local_operation_id,
            )?;
            let executed = execute_transaction(
                &transaction,
                &store.integrity,
                &store.configuration,
                &store.topology,
                &store.replica_id,
                &request,
                ReceiptMetadata {
                    kind: ReceiptKind::AuthenticatedImport,
                    source_replica_id: Some(snapshot.replica_id.clone()),
                    wire_operation_id: Some(wire_operation_id.into()),
                },
            )?;
            let receipt = executed.receipt.as_ref().ok_or_else(|| {
                DurableError::Corrupt("authenticated import has no receipt".into())
            })?;
            audit.local_receipt_operation_id = Some(receipt.operation_id.clone());
            audit.local_receipt_sha256 = Some(receipt.sha256.clone());
            audit.replayed = executed.replayed;
            let evidence = insert_audit(
                &transaction,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                &audit,
                true,
            )?;
            store.integrity.commit(transaction)?;
            Ok(AuthenticatedImport {
                executed,
                audit: evidence,
            })
        })
    }

    /// Records one bounded exchange phase after validating any local receipt link.
    /// Identical event replay returns the original evidence; changed reuse refuses.
    ///
    /// # Errors
    /// Rejects invalid fields, corrupt state, receipt mismatches and failed commits.
    pub fn record_exchange_audit(
        &mut self,
        audit: &ExchangeAuditEvent,
    ) -> DurableResult<ExchangeAuditEvidence> {
        if audit.phase == AuditPhase::InboundImportCommitted {
            return Err(DurableError::InvalidAudit(
                "inbound import audit must use the atomic authenticated-import API".into(),
            ));
        }
        self.guarded(|store| {
            let transaction = begin_verified(
                &mut store.connection,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                TransactionBehavior::Immediate,
            )?;
            let evidence = insert_audit(
                &transaction,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                audit,
                false,
            )?;
            store.integrity.commit(transaction)?;
            Ok(evidence)
        })
    }
}

fn lock<'a, T>(mutex: &'a Mutex<T>, label: &str) -> DurableResult<MutexGuard<'a, T>> {
    mutex
        .lock()
        .map_err(|_| DurableError::Storage(format!("{label} poisoned")))
}

fn integrity_state(entry: &StoreIntegrityEntry) -> DurableResult<MutexGuard<'_, IntegrityState>> {
    lock(&entry.state, "store integrity lock")
}

impl StoreIntegrityEntry {
    fn refuse_if_failed(&self) -> DurableResult<()> {
        integrity_state(self)?.failure.clone().map_or(Ok(()), Err)
    }

    /// The verified positions and the number of complete verifications so far.
    fn positions(&self) -> DurableResult<(Option<VerifiedPositions>, u64)> {
        let state = integrity_state(self)?;
        Ok((state.positions.clone(), state.full_verifications))
    }

    /// Records the first verification failure; every later operation returns it.
    fn fail_closed(&self, problem: DurableError) -> DurableError {
        if let Ok(mut state) = self.state.lock() {
            state.failure.get_or_insert_with(|| problem.clone());
        }
        problem
    }

    /// Records a corrupt stored state as a verification failure and returns any
    /// other error unchanged.
    fn fail_closed_if_corrupt(&self, problem: DurableError) -> DurableError {
        match problem {
            DurableError::Corrupt(_) => self.fail_closed(problem),
            _ => problem,
        }
    }

    /// Commits a transaction on this database file unless the file has failed
    /// closed. The integrity state stays locked while SQLite commits, so a
    /// failure is recorded either before the commit, which then rolls back and
    /// returns that failure, or once the commit has completed: no transaction of
    /// the process commits after the file is closed, including one that began,
    /// or waited for the write lock, before the failure was recorded.
    fn commit(&self, transaction: Transaction<'_>) -> DurableResult<()> {
        let state = integrity_state(self)?;
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        transaction.commit().map_err(error)
    }

    /// Returns stored rows an operation read at use, inside a transaction that
    /// [`begin_verified`] opened. That transaction already holds its snapshot
    /// (and a write transaction its write lock), and SQLite reports a busy or
    /// locked database only while a transaction takes those, never on a read
    /// inside a snapshot it holds. A failure here therefore concerns the stored
    /// rows themselves: bytes that could not be read (`storage`) or rows that do
    /// not verify (`corrupt`), and it closes the store as a failed verification of
    /// those rows does. The operation's own writes and commit are not read
    /// through this, so a full disk closes nothing.
    fn read_at_use<T>(&self, read: DurableResult<T>) -> DurableResult<T> {
        read.map_err(|problem| self.fail_closed(problem))
    }

    fn record_full_verification(
        &self,
        verified: VerifiedPositions,
        started: Instant,
        replace_positions: bool,
    ) -> DurableResult<()> {
        let mut state = integrity_state(self)?;
        let positions = match (replace_positions, state.positions.take()) {
            (false, Some(current)) => current.furthest(verified),
            _ => verified,
        };
        state.positions = Some(positions);
        state.last_full_verification = Some(
            state
                .last_full_verification
                .map_or(started, |previous| previous.max(started)),
        );
        state.full_verifications = state.full_verifications.saturating_add(1);
        Ok(())
    }

    fn advance(&self, verified: VerifiedPositions) -> DurableResult<()> {
        let mut state = integrity_state(self)?;
        let positions = match state.positions.take() {
            Some(current) => current.furthest(verified),
            None => verified,
        };
        state.positions = Some(positions);
        Ok(())
    }
}

impl VerifiedPositions {
    fn furthest(self, other: Self) -> Self {
        Self {
            facts: self.facts.furthest(other.facts),
            receipts: self.receipts.furthest(other.receipts),
            audits: self.audits.furthest(other.audits),
        }
    }

    fn tables(&self) -> [(AppendOnlyTable, &TablePosition); 3] {
        [
            (AppendOnlyTable::Facts, &self.facts),
            (AppendOnlyTable::Receipts, &self.receipts),
            (AppendOnlyTable::Audits, &self.audits),
        ]
    }
}

impl TablePosition {
    fn furthest(self, other: Self) -> Self {
        if other.rowid > self.rowid {
            other
        } else {
            self
        }
    }
}

/// The SQLite connection of one store open, with what the open learnt about the
/// database file it names.
struct OpenedFile {
    connection: Connection,
    /// Declared after `connection`: dropped once SQLite has closed it.
    counted: LiveConnection,
    integrity: Arc<StoreIntegrityEntry>,
    /// This connection is the only one this process has on the path, so the file
    /// was preflighted through a private copy before SQLite opened it.
    first_connection: bool,
}

/// Opens the connection of a store and counts it on its path. An existing file is
/// copied for a read-only preflight of its schema and identity only while this
/// process holds no connection on the path, and only when the process has not
/// preflighted that file state before; while it has the store open, the path must
/// name the file that is open and the open's own transaction does the checking.
fn open_counted_connection(
    path: &Path,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<OpenedFile> {
    let live = live_file(path)?;
    // Held until this connection is counted and the file it opened is known: no copy
    // of the files starts while it opens, and it does not open while one runs.
    let mut open_file = lock(&live.gate, "live store lock")?;
    let first_connection = live.connections.load(Ordering::SeqCst) == 0;
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => None,
        Err(problem) => return Err(error(problem)),
    };
    // The integrity entry of an existing file is known before its preflight: a file
    // this process closed is refused without being copied, and a preflight that
    // finds a corrupt schema closes the file.
    let existing = match &before {
        Some(metadata) => {
            require_regular_nonsymlink(path)?;
            let integrity = integrity_entry(metadata, topology, replica_id)?;
            integrity.refuse_if_failed()?;
            if first_connection {
                let key = preflight_key(metadata, topology, replica_id)?;
                if !preflighted_stores()?.contains(&key) {
                    preflight_existing_store(path, topology, replica_id)
                        .map_err(|problem| integrity.fail_closed_if_corrupt(problem))?;
                }
            } else if *open_file != Some(file_identity(metadata)) {
                // Its sidecars may still be those of the open file: it is neither
                // copied nor opened.
                return Err(changed_live_file());
            }
            Some(integrity)
        }
        None if !first_connection => return Err(changed_live_file()),
        None => None,
    };
    let counted = LiveConnection::count(&live);
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    let current = fs::symlink_metadata(path).map_err(error)?;
    if let Some(before) = &before {
        if first_connection {
            if preflight_key(&current, topology, replica_id)?
                != preflight_key(before, topology, replica_id)?
            {
                return Err(DurableError::Storage(
                    "manager store path changed after read-only preflight".into(),
                ));
            }
        } else if file_identity(&current) != file_identity(before) {
            return Err(changed_live_file());
        }
    }
    if first_connection {
        *open_file = Some(file_identity(&current));
    }
    drop(open_file);
    let integrity = match existing {
        Some(integrity) => integrity,
        None => integrity_entry(&current, topology, replica_id)?,
    };
    Ok(OpenedFile {
        connection,
        counted,
        integrity,
        first_connection,
    })
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    let birth = metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(UNIX_EPOCH).ok())
        .map(|since| (since.as_secs(), since.subsec_nanos()));
    (metadata.dev(), metadata.ino(), birth)
}

fn live_file(path: &Path) -> DurableResult<Arc<LiveFile>> {
    let name = path
        .file_name()
        .ok_or_else(|| DurableError::Storage("manager store path names no file".into()))?;
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let directory = fs::metadata(directory).map_err(error)?;
    let mut files = lock(
        LIVE_FILES.get_or_init(|| Mutex::new(BTreeMap::new())),
        "live store registry lock",
    )?;
    Ok(Arc::clone(
        files
            .entry((directory.dev(), directory.ino(), name.to_os_string()))
            .or_default(),
    ))
}

fn changed_live_file() -> DurableError {
    DurableError::Storage(
        "manager store path no longer names the database file this process has open".into(),
    )
}

fn integrity_entry(
    metadata: &fs::Metadata,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<Arc<StoreIntegrityEntry>> {
    let (device, inode, birth) = file_identity(metadata);
    let key = (
        device,
        inode,
        birth,
        replica_id.to_string(),
        json(topology)?,
    );
    let mut stores = lock(
        VERIFIED_STORES.get_or_init(|| Mutex::new(BTreeMap::new())),
        "verified store registry lock",
    )?;
    Ok(Arc::clone(stores.entry(key).or_default()))
}

/// Takes the transaction's read snapshot with a statement that cannot fail on
/// stored content, so a busy or unavailable store does not count as a failed
/// verification.
fn acquire_snapshot(connection: &Connection) -> DurableResult<()> {
    connection
        .pragma_query_value(None, "schema_version", |row| row.get::<_, i64>(0))
        .map_err(error)?;
    Ok(())
}

/// Opens a transaction verified under the store integrity model: the schema
/// shape, the last verified row of each table, and every row appended since.
/// A changed verified row, or a store without verified positions, is verified
/// completely inside the transaction instead.
fn begin_verified<'c>(
    connection: &'c mut Connection,
    integrity: &StoreIntegrityEntry,
    topology: &Topology,
    replica_id: &str,
    behavior: TransactionBehavior,
) -> DurableResult<Transaction<'c>> {
    integrity.refuse_if_failed()?;
    // Read before the snapshot exists: every row these positions name was
    // committed before it, so a missing or different row was changed in place.
    let (positions, _) = integrity.positions()?;
    let transaction = connection
        .transaction_with_behavior(behavior)
        .map_err(error)?;
    acquire_snapshot(&transaction)?;
    verify_immutable_schema(&transaction).map_err(|problem| integrity.fail_closed(problem))?;
    let unchanged = match &positions {
        Some(positions) => positions_unchanged(&transaction, positions)
            .map_err(|problem| integrity.fail_closed(problem))?,
        None => false,
    };
    match positions {
        Some(positions) if unchanged => {
            let verified = verify_appended_rows(&transaction, topology, replica_id, &positions)
                .map_err(|problem| integrity.fail_closed(problem))?;
            integrity.advance(verified)?;
        }
        _ => {
            let started = Instant::now();
            let verified = full_verification(&transaction, topology, replica_id)
                .map_err(|problem| integrity.fail_closed(problem))?;
            integrity.record_full_verification(verified, started, true)?;
        }
    }
    Ok(transaction)
}

fn run_full_verification(
    connection: &mut Connection,
    integrity: &StoreIntegrityEntry,
    topology: &Topology,
    replica_id: &str,
    replace_positions: bool,
) -> DurableResult<()> {
    let started = Instant::now();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(error)?;
    acquire_snapshot(&transaction)?;
    let verified = full_verification(&transaction, topology, replica_id)
        .map_err(|problem| integrity.fail_closed(problem))?;
    integrity.commit(transaction)?;
    integrity.record_full_verification(verified, started, replace_positions)
}

/// Verifies every stored row and returns the last row of each table in the
/// same snapshot.
fn full_verification(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<VerifiedPositions> {
    verify_all(connection, topology, replica_id)?;
    Ok(VerifiedPositions {
        facts: table_tail(connection, AppendOnlyTable::Facts)?,
        receipts: table_tail(connection, AppendOnlyTable::Receipts)?,
        audits: table_tail(connection, AppendOnlyTable::Audits)?,
    })
}

fn table_tail(connection: &Connection, table: AppendOnlyTable) -> DurableResult<TablePosition> {
    Ok(connection
        .query_row(
            &format!(
                "SELECT rowid, sha256 FROM {} ORDER BY rowid DESC LIMIT 1",
                table.name()
            ),
            [],
            |row| {
                Ok(TablePosition {
                    rowid: row.get(0)?,
                    sha256: Some(row.get(1)?),
                })
            },
        )
        .optional()
        .map_err(error)?
        .unwrap_or_default())
}

/// The rowid rule of the append-only tables, checked by a complete verification.
///
/// The Store never names a rowid, never deletes a row, and the triggers refuse
/// `DELETE`, so SQLite numbers the rows of each table 1, 2, 3, … in insertion
/// order: a plain insert takes the largest rowid plus one, and a failed statement
/// or a rolled-back transaction removes its row, so the next insert reuses that
/// rowid. A table whose rows are not exactly the rowids 1 to their count was
/// written by something other than the Store, and a verified prefix of it would
/// no longer be every row at or below its last verified row.
fn verify_contiguous_rowids(connection: &Connection, table: AppendOnlyTable) -> DurableResult<()> {
    let (count, first, last) = connection
        .query_row(
            &format!(
                "SELECT count(*), min(rowid), max(rowid) FROM {}",
                table.name()
            ),
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .map_err(error)?;
    if count > 0 && (first != Some(1) || last != Some(count)) {
        return Err(rowid_gap(table));
    }
    Ok(())
}

/// The rowid rule checked by every transaction: no row precedes rowid 1. With
/// the rows up to the last verified row known to be exactly 1 to its rowid, a row
/// that any writer adds without removing a stored row either has a rowid at or
/// below 0, refused here in one seek of the table, or follows the last verified
/// row, where [`next_rowid`] refuses a gap before the row is verified.
fn verify_first_rowid(connection: &Connection, table: AppendOnlyTable) -> DurableResult<()> {
    let first: Option<i64> = connection
        .query_row(
            &format!("SELECT min(rowid) FROM {}", table.name()),
            [],
            |row| row.get(0),
        )
        .map_err(error)?;
    if first.is_some_and(|first| first != 1) {
        return Err(rowid_gap(table));
    }
    Ok(())
}

/// Requires an appended row to continue its table's rowids without a gap.
fn next_rowid(table: AppendOnlyTable, previous: &TablePosition, rowid: i64) -> DurableResult<()> {
    if previous.rowid.checked_add(1) == Some(rowid) {
        Ok(())
    } else {
        Err(rowid_gap(table))
    }
}

fn rowid_gap(table: AppendOnlyTable) -> DurableError {
    DurableError::Corrupt(format!(
        "stored {} rowids are not contiguous from 1",
        table.name()
    ))
}

/// Whether the last verified row of every table is still present with the same
/// stored checksum.
fn positions_unchanged(
    connection: &Connection,
    positions: &VerifiedPositions,
) -> DurableResult<bool> {
    for (table, position) in positions.tables() {
        if position.rowid == 0 {
            continue;
        }
        let stored: Option<String> = connection
            .query_row(
                &format!("SELECT sha256 FROM {} WHERE rowid=?1", table.name()),
                [position.rowid],
                |row| row.get(0),
            )
            .optional()
            .map_err(error)?;
        if stored != position.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Verifies only the rows appended after the verified positions, with the same
/// per-row checks as a complete verification, and the complete audit sequence of
/// every exchange attempt those rows belong to. No table may have a row before
/// rowid 1, and the appended rows must continue each table's rowids.
fn verify_appended_rows(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    positions: &VerifiedPositions,
) -> DurableResult<VerifiedPositions> {
    for table in AppendOnlyTable::ALL {
        verify_first_rowid(connection, table)?;
    }
    let receipts = verify_appended_receipts(
        connection,
        topology.logical_manager_id(),
        &positions.receipts,
    )?;
    let facts = verify_appended_facts(connection, topology, replica_id, &positions.facts)?;
    let audits = verify_appended_audits(connection, topology, replica_id, &positions.audits)?;
    Ok(VerifiedPositions {
        facts,
        receipts,
        audits,
    })
}

/// Creates schema v3 and the identity row in an empty database, or checks the
/// schema version, identity and schema shape of an existing one.
fn initialize_or_check_identity(
    transaction: &Transaction<'_>,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<()> {
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
            return Err(DurableError::Refused(RefusalReason::UnrecognizedStore));
        }
        transaction.execute_batch(SCHEMA).map_err(error)?;
        let inserted = transaction
            .execute(
                "INSERT INTO identity VALUES (1, ?1, ?2)",
                params![replica_id, json(topology)?],
            )
            .map_err(error)?;
        if inserted != 1 {
            return Err(DurableError::Storage(
                "identity insert did not affect exactly one row".into(),
            ));
        }
    } else if version != 3 {
        return Err(DurableError::Refused(RefusalReason::UnsupportedSchema));
    }
    let identity: (String, String) = transaction
        .query_row(
            "SELECT replica_id, topology_json FROM identity WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(error)?;
    if identity != (replica_id.to_string(), json(topology)?) {
        return Err(DurableError::Refused(RefusalReason::IdentityMismatch));
    }
    verify_immutable_schema(transaction)
}

fn preflighted_stores() -> DurableResult<MutexGuard<'static, BTreeSet<PreflightKey>>> {
    lock(
        PREFLIGHTED_STORES.get_or_init(|| Mutex::new(BTreeSet::new())),
        "preflight cache lock",
    )
}

/// Records a preflighted file state and forgets earlier states of the same file
/// identity, which a file cannot return to.
fn remember_preflighted(key: PreflightKey) -> DurableResult<()> {
    let mut stores = preflighted_stores()?;
    stores.retain(|known| (known.0, known.1, &known.7, &known.8) != (key.0, key.1, &key.7, &key.8));
    stores.insert(key);
    Ok(())
}

fn preflight_key(
    metadata: &fs::Metadata,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<PreflightKey> {
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        replica_id.into(),
        json(topology)?,
    ))
}

fn preflight_existing_store(
    path: &Path,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<()> {
    require_regular_nonsymlink(path)?;
    let (_snapshot, connection) = open_read_only_snapshot(path)?;
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(error)?;
    if version == 0 {
        let tables: u32 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .map_err(error)?;
        return if tables == 0 {
            Ok(())
        } else {
            Err(DurableError::Refused(RefusalReason::UnrecognizedStore))
        };
    }
    if version != 3 {
        return Err(DurableError::Refused(RefusalReason::UnsupportedSchema));
    }
    verify_immutable_schema(&connection)?;
    let identity: (String, String) = connection
        .query_row(
            "SELECT replica_id, topology_json FROM identity WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(error)?;
    if identity != (replica_id.into(), json(topology)?) {
        return Err(DurableError::Refused(RefusalReason::IdentityMismatch));
    }
    Ok(())
}

fn execute_transaction(
    transaction: &Transaction<'_>,
    integrity: &StoreIntegrityEntry,
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
        // A replay returns stored bytes, so that receipt row is verified first.
        let prior = integrity.read_at_use(stored_receipt(
            transaction,
            topology.logical_manager_id(),
            id,
        ))?;
        if let Some(prior) = prior {
            if prior.request_json != request_json
                || prior.kind != enum_text(&receipt_metadata.kind)?
                || prior.source_replica_id != receipt_metadata.source_replica_id
                || prior.wire_operation_id != receipt_metadata.wire_operation_id
            {
                return Err(DurableError::Refused(RefusalReason::OperationIdReused));
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
    let mut replica = integrity.read_at_use(load(transaction, topology, replica_id))?;
    let before = replica.history.clone();
    let response = apply(configuration, &mut replica, request)?;
    for (id, fact) in &replica.history {
        if !before.contains_key(id) {
            let encoded = json(fact)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO facts(event_id, fact_json, sha256) VALUES (?1, ?2, ?3)",
                    params![id, encoded, digest(&encoded)],
                )
                .map_err(error)?;
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
                "INSERT INTO receipts (operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, enum_text(&receipt_metadata.kind)?, receipt_metadata.source_replica_id, receipt_metadata.wire_operation_id, request_json, response_json, sha256],
            )
            .map_err(error)?;
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ReceiptMetadata {
    kind: ReceiptKind,
    source_replica_id: Option<String>,
    wire_operation_id: Option<String>,
}

fn receipt_metadata_for_request(request: &Request) -> ReceiptMetadata {
    match request {
        Request::Observe { .. } => ReceiptMetadata {
            kind: ReceiptKind::Observe,
            source_replica_id: None,
            wire_operation_id: None,
        },
        Request::Import { snapshot, .. } => ReceiptMetadata {
            kind: ReceiptKind::LaboratoryImport,
            source_replica_id: Some(snapshot.replica_id.clone()),
            wire_operation_id: None,
        },
        Request::Export {} | Request::Inspect {} | Request::CheckService { .. } => {
            ReceiptMetadata {
                kind: ReceiptKind::Observe,
                source_replica_id: None,
                wire_operation_id: None,
            }
        }
    }
}

/// Returns the bounded destination-local receipt key for one authenticated
/// peer operation. The wire operation remains available separately in audit
/// and receipt metadata.
///
/// # Errors
/// Refuses invalid source or operation tokens and reports serialization failure.
pub fn authenticated_import_receipt_id(
    topology: &Topology,
    source_replica_id: &str,
    wire_operation_id: &str,
) -> DurableResult<String> {
    validate_token("source replica ID", source_replica_id)?;
    validate_token("wire operation ID", wire_operation_id)?;
    authenticated_import_receipt_id_parts(
        topology.logical_manager_id(),
        source_replica_id,
        wire_operation_id,
    )
}

fn authenticated_import_receipt_id_parts(
    logical_manager_id: &str,
    source_replica_id: &str,
    wire_operation_id: &str,
) -> DurableResult<String> {
    let material = json(&(
        "podmesh-manager-ha-network-import/1",
        logical_manager_id,
        source_replica_id,
        wire_operation_id,
    ))?;
    Ok(format!("network:{}", digest(&material)))
}

impl Store {
    /// Returns the highest producer sequence among the facts this store appended
    /// itself, as the responses of its `observe` receipts record them, or `None`
    /// when it appended none. A fact of this replica's own origin that the store
    /// imported from a peer has no `observe` receipt and is not counted. The
    /// receipts are read, and each verified, in one verified read transaction.
    ///
    /// # Errors
    /// Returns the transaction's errors, and `corrupt` for an `observe` receipt
    /// whose row or response does not verify.
    pub fn highest_local_observation(&mut self) -> DurableResult<Option<u64>> {
        self.guarded(|store| {
            let transaction = begin_verified(
                &mut store.connection,
                &store.integrity,
                &store.topology,
                &store.replica_id,
                TransactionBehavior::Deferred,
            )?;
            // The observe receipts are rows read at use: one that cannot be read or does
            // not verify closes the store, as for every other read at use.
            let highest = store.integrity.read_at_use((|| {
                let mut statement = transaction
                    .prepare(&format!(
                        "SELECT {RECEIPT_COLUMNS} FROM receipts WHERE kind = ?1"
                    ))
                    .map_err(error)?;
                let rows = statement
                    .query_map([enum_text(&ReceiptKind::Observe)?], read_receipt_row)
                    .map_err(error)?;
                let mut highest = None;
                for row in rows {
                    let receipt = row.map_err(error)?;
                    verify_receipt_row(store.topology.logical_manager_id(), &receipt)?;
                    let Ok(Response::Observed { fact }) =
                        serde_json::from_str(&receipt.response_json)
                    else {
                        return Err(DurableError::Corrupt(
                            "stored observe receipt does not record an observation".into(),
                        ));
                    };
                    if fact.origin_replica_id == store.replica_id {
                        highest = highest.max(Some(fact.producer_sequence));
                    }
                }
                Ok(highest)
            })())?;
            store.integrity.commit(transaction)?;
            Ok(highest)
        })
    }
}

fn mutation_operation_id(request: &Request) -> DurableResult<Option<&String>> {
    match request {
        Request::Observe { operation_id, .. } => {
            validate_token("operation ID", operation_id)
                .map_err(|_| DurableError::Refused(RefusalReason::InvalidRequest))?;
            Ok(Some(operation_id))
        }
        Request::Import { operation_id, .. } => {
            validate_laboratory_operation_id(operation_id)?;
            Ok(Some(operation_id))
        }
        _ => Ok(None),
    }
}

fn validate_laboratory_operation_id(operation_id: &str) -> DurableResult<()> {
    if operation_id.is_empty()
        || operation_id.len() > 128
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
    {
        return Err(DurableError::Refused(RefusalReason::InvalidRequest));
    }
    Ok(())
}

fn validate_receipt_operation_id(operation_id: &str, kind: ReceiptKind) -> DurableResult<()> {
    let reserved = operation_id.starts_with("network:");
    if reserved != (kind == ReceiptKind::AuthenticatedImport) {
        return Err(DurableError::Refused(RefusalReason::InvalidRequest));
    }
    Ok(())
}

fn apply(
    configuration: &Configuration,
    replica: &mut Replica,
    request: &Request,
) -> DurableResult<Response> {
    match request {
        Request::Observe {
            scope,
            subject,
            exclusive_resource,
            active_claim,
            value,
            ..
        } => Ok(Response::Observed {
            fact: replica
                .observe(
                    scope,
                    subject,
                    exclusive_resource.as_deref(),
                    *active_claim,
                    value,
                )
                .map_err(refused_policy)?,
        }),
        Request::Import { snapshot, .. } => {
            let peer = restore_snapshot(&replica.topology, snapshot)?;
            let before = replica.history_len();
            for fact in peer.history.into_values() {
                replica.ingest(fact).map_err(refused_policy)?;
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
                .collect::<DurableResult<_>>()?;
            // Include current local state, never substitute an old caller-supplied local copy.
            let mut copies = vec![&*replica];
            copies.extend(peers.iter());
            let reconciled = Reconciliation::after_full_exchange(&replica.topology, &copies)
                .map_err(refused_policy)?;
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

fn restore_snapshot(topology: &Topology, snapshot: &Snapshot) -> DurableResult<Replica> {
    if snapshot.configuration.topology().map_err(refused_policy)? != *topology {
        return Err(DurableError::Refused(RefusalReason::PolicyViolation));
    }
    let mut replica = topology
        .instantiate(&snapshot.replica_id)
        .map_err(refused_policy)?;
    for fact in &snapshot.facts {
        replica.ingest(fact.clone()).map_err(refused_policy)?;
    }
    Ok(replica)
}

fn load(connection: &Connection, topology: &Topology, id: &str) -> DurableResult<Replica> {
    let mut replica = topology.instantiate(id).map_err(corrupt_model)?;
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
        replica
            .ingest(decode_fact_row(&id, &encoded, &hash)?)
            .map_err(corrupt_model)?;
    }
    Ok(replica)
}

fn decode_fact_row(id: &str, encoded: &str, hash: &str) -> DurableResult<Fact> {
    if digest(encoded) != hash {
        return Err(DurableError::Corrupt("stored fact hash mismatch".into()));
    }
    let fact: Fact = serde_json::from_str(encoded)
        .map_err(|_| DurableError::Corrupt("stored fact JSON is invalid".into()))?;
    if fact.event_id != id {
        return Err(DurableError::Corrupt(
            "stored event identity mismatch".into(),
        ));
    }
    Ok(fact)
}

/// Verifies facts appended after `from` with the per-fact checks of `load`: each
/// is ingested into an empty replica, which applies the same topology, identity
/// and sequence validation per fact.
fn verify_appended_facts(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    from: &TablePosition,
) -> DurableResult<TablePosition> {
    let mut replica = topology.instantiate(replica_id).map_err(corrupt_model)?;
    let mut statement = connection
        .prepare(
            "SELECT rowid, event_id, fact_json, sha256 FROM facts WHERE rowid > ?1 ORDER BY rowid",
        )
        .map_err(error)?;
    let rows = statement
        .query_map([from.rowid], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(error)?;
    let mut position = from.clone();
    for row in rows {
        let (rowid, id, encoded, hash) = row.map_err(error)?;
        next_rowid(AppendOnlyTable::Facts, &position, rowid)?;
        replica
            .ingest(decode_fact_row(&id, &encoded, &hash)?)
            .map_err(corrupt_model)?;
        position = TablePosition {
            rowid,
            sha256: Some(hash),
        };
    }
    Ok(position)
}

fn json(value: &impl Serialize) -> DurableResult<String> {
    serde_json::to_string(value).map_err(error)
}

fn refused_policy(value: impl std::fmt::Display) -> DurableError {
    let _ = value;
    DurableError::Refused(RefusalReason::PolicyViolation)
}

fn invalid_configuration(value: impl std::fmt::Display) -> DurableError {
    DurableError::Storage(format!("invalid local configuration: {value}"))
}

fn validate_local_configuration(
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<Topology> {
    let topology = configuration.topology().map_err(invalid_configuration)?;
    topology
        .instantiate(replica_id)
        .map_err(invalid_configuration)?;
    Ok(topology)
}

fn corrupt_model(value: impl std::fmt::Display) -> DurableError {
    DurableError::Corrupt(format!("stored model state is invalid: {value}"))
}

fn error(value: impl std::fmt::Display) -> DurableError {
    DurableError::Storage(value.to_string())
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

// A domain-separated JSON tuple is unambiguous even when fields contain quotes,
// delimiters, newlines or NULs. The checksum binds exact stored JSON strings,
// including whitespace; it is an integrity check, not origin authentication.
fn receipt_digest(
    logical_manager_id: &str,
    operation_id: &str,
    metadata: &ReceiptMetadata,
    request_json: &str,
    response_json: &str,
) -> DurableResult<String> {
    Ok(digest(&json(&(
        "podmesh-manager-ha-receipt/2",
        logical_manager_id,
        operation_id,
        metadata,
        request_json,
        response_json,
    ))?))
}

const RECEIPT_COLUMNS: &str =
    "operation_id, kind, source_replica_id, wire_operation_id, request_json, response_json, sha256";

struct StoredReceipt {
    operation_id: String,
    kind: String,
    source_replica_id: Option<String>,
    wire_operation_id: Option<String>,
    request_json: String,
    response_json: String,
    sha256: String,
}

fn read_receipt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredReceipt> {
    Ok(StoredReceipt {
        operation_id: row.get(0)?,
        kind: row.get(1)?,
        source_replica_id: row.get(2)?,
        wire_operation_id: row.get(3)?,
        request_json: row.get(4)?,
        response_json: row.get(5)?,
        sha256: row.get(6)?,
    })
}

fn verify_receipt_row(logical_manager_id: &str, receipt: &StoredReceipt) -> DurableResult<()> {
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

/// Reads the stored receipt of one operation ID, if there is one, and verifies it.
fn stored_receipt(
    connection: &Connection,
    logical_manager_id: &str,
    operation_id: &str,
) -> DurableResult<Option<StoredReceipt>> {
    let receipt = connection
        .query_row(
            &format!("SELECT {RECEIPT_COLUMNS} FROM receipts WHERE operation_id=?1"),
            [operation_id],
            read_receipt_row,
        )
        .optional()
        .map_err(error)?;
    if let Some(receipt) = &receipt {
        verify_receipt_row(logical_manager_id, receipt)?;
    }
    Ok(receipt)
}

fn verify_receipts(connection: &Connection, logical_manager_id: &str) -> DurableResult<()> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {RECEIPT_COLUMNS} FROM receipts ORDER BY operation_id"
        ))
        .map_err(error)?;
    let rows = statement.query_map([], read_receipt_row).map_err(error)?;
    for row in rows {
        verify_receipt_row(logical_manager_id, &row.map_err(error)?)?;
    }
    Ok(())
}

fn verify_appended_receipts(
    connection: &Connection,
    logical_manager_id: &str,
    from: &TablePosition,
) -> DurableResult<TablePosition> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {RECEIPT_COLUMNS}, rowid FROM receipts WHERE rowid > ?1 ORDER BY rowid"
        ))
        .map_err(error)?;
    let rows = statement
        .query_map([from.rowid], |row| {
            Ok((read_receipt_row(row)?, row.get::<_, i64>(7)?))
        })
        .map_err(error)?;
    let mut position = from.clone();
    for row in rows {
        let (receipt, rowid) = row.map_err(error)?;
        next_rowid(AppendOnlyTable::Receipts, &position, rowid)?;
        verify_receipt_row(logical_manager_id, &receipt)?;
        position = TablePosition {
            rowid,
            sha256: Some(receipt.sha256),
        };
    }
    Ok(position)
}

fn validate_receipt_metadata(
    logical_manager_id: &str,
    operation_id: &str,
    request_json: &str,
    metadata: &ReceiptMetadata,
) -> DurableResult<()> {
    let request: Request = serde_json::from_str(request_json)
        .map_err(|_| DurableError::Corrupt("stored receipt request is invalid".into()))?;
    match (&request, metadata.kind) {
        (
            Request::Observe {
                operation_id: stored,
                ..
            },
            ReceiptKind::Observe,
        ) if stored == operation_id
            && !operation_id.starts_with("network:")
            && metadata.source_replica_id.is_none()
            && metadata.wire_operation_id.is_none() => {}
        (
            Request::Import {
                operation_id: stored,
                snapshot,
            },
            ReceiptKind::LaboratoryImport,
        ) if stored == operation_id
            && !operation_id.starts_with("network:")
            && metadata.source_replica_id.as_deref() == Some(&snapshot.replica_id)
            && metadata.wire_operation_id.is_none() => {}
        (
            Request::Import {
                operation_id: stored,
                snapshot,
            },
            ReceiptKind::AuthenticatedImport,
        ) if stored == operation_id
            && operation_id.starts_with("network:")
            && metadata.source_replica_id.as_deref() == Some(&snapshot.replica_id)
            && metadata.wire_operation_id.is_some()
            && authenticated_import_receipt_id_parts(
                logical_manager_id,
                &snapshot.replica_id,
                metadata.wire_operation_id.as_deref().unwrap_or_default(),
            )?
            .as_str()
                == operation_id => {}
        _ => {
            return Err(DurableError::Corrupt(
                "stored receipt kind or source does not match its request".into(),
            ));
        }
    }
    Ok(())
}

fn verify_all(connection: &Connection, topology: &Topology, replica_id: &str) -> DurableResult<()> {
    verify_immutable_schema(connection)?;
    for table in AppendOnlyTable::ALL {
        verify_contiguous_rowids(connection, table)?;
    }
    verify_receipts(connection, topology.logical_manager_id())?;
    load(connection, topology, replica_id)?;
    verify_audits(connection, topology, replica_id)?;
    Ok(())
}

fn verify_immutable_schema(connection: &Connection) -> DurableResult<()> {
    let expected = EXPECTED_SCHEMA_SHAPE.get_or_init(|| {
        let connection = Connection::open_in_memory()
            .expect("constant manager schema must open an in-memory SQLite database");
        connection
            .execute_batch(SCHEMA)
            .expect("constant manager schema must initialize an in-memory SQLite database");
        schema_shape(&connection).expect("constant manager schema must be inspectable")
    });
    if schema_shape(connection)? != *expected {
        return Err(DurableError::Corrupt(
            "manager store schema shape mismatch".into(),
        ));
    }
    Ok(())
}

fn schema_shape(connection: &Connection) -> DurableResult<Vec<(String, String, String)>> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, sql FROM sqlite_master
             WHERE substr(name, 1, 7) <> 'sqlite_' AND sql IS NOT NULL
             ORDER BY type, name",
        )
        .map_err(error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                normalize_sql(&row.get::<_, String>(2)?),
            ))
        })
        .map_err(error)?
        .map(|row| row.map_err(error))
        .collect();
    rows
}

fn normalize_sql(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(';')
        .to_ascii_lowercase()
}

fn validate_token(_label: &str, value: &str) -> DurableResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err(DurableError::Refused(RefusalReason::InvalidRequest));
    }
    Ok(())
}

fn validate_attempt_id(value: &str) -> DurableResult<()> {
    let Some(hash) = value.strip_prefix("attempt:") else {
        return Err(DurableError::InvalidAudit(
            "attempt ID must use the locally generated attempt:<sha256> format".into(),
        ));
    };
    validate_hash("attempt digest", hash)
        .map_err(|_| DurableError::InvalidAudit("attempt ID digest is invalid".into()))
}

fn validate_wire_nonce_value(value: &str) -> DurableResult<()> {
    if let Some(hash) = value.strip_prefix("preauth:") {
        validate_hash("pre-authentication nonce digest", hash)
    } else {
        validate_token("wire nonce", value)
    }
}

fn validate_optional_token(label: &str, value: Option<&String>) -> DurableResult<()> {
    value.map_or(Ok(()), |value| validate_token(label, value))
}

fn validate_hash(_label: &str, value: &str) -> DurableResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DurableError::Refused(RefusalReason::InvalidRequest));
    }
    Ok(())
}

fn validate_optional_hash(label: &str, value: Option<&String>) -> DurableResult<()> {
    value.map_or(Ok(()), |value| validate_hash(label, value))
}

fn validate_audit(event: &ExchangeAuditEvent) -> DurableResult<()> {
    validate_audit_fields(event)
        .and_then(|()| validate_audit_semantics(event))
        .map_err(|problem| match problem {
            DurableError::InvalidAudit(_) => problem,
            _ => DurableError::InvalidAudit(problem.to_string()),
        })
}

fn validate_audit_fields(event: &ExchangeAuditEvent) -> DurableResult<()> {
    let audit_token = |label, value: &String| {
        validate_token(label, value)
            .map_err(|problem| DurableError::InvalidAudit(problem.to_string()))
    };
    audit_token("audit event ID", &event.audit_event_id)?;
    validate_attempt_id(&event.attempt_id)?;
    validate_audit_wire_nonce(event)?;
    validate_optional_token(
        "authenticated peer identity",
        event.authenticated_peer_id.as_ref(),
    )?;
    validate_optional_token("peer claim", event.peer_claim.as_ref())?;
    validate_optional_token("operation ID", event.operation_id.as_ref())?;
    validate_optional_hash("request digest", event.request_sha256.as_ref())?;
    validate_optional_hash("reply digest", event.reply_sha256.as_ref())?;
    validate_optional_token(
        "local receipt operation ID",
        event.local_receipt_operation_id.as_ref(),
    )?;
    validate_optional_hash("local receipt digest", event.local_receipt_sha256.as_ref())?;
    validate_optional_token(
        "remote receipt operation ID",
        event.remote_receipt_operation_id.as_ref(),
    )?;
    validate_optional_hash(
        "remote receipt digest",
        event.remote_receipt_sha256.as_ref(),
    )?;
    for count in [
        event.request_frame_bytes,
        event.request_announced_body_bytes.unwrap_or_default(),
        event.reply_frame_bytes,
        event.reply_announced_body_bytes.unwrap_or_default(),
    ] {
        i64::try_from(count)
            .map_err(|_| DurableError::InvalidAudit("byte count exceeds SQLite range".into()))?;
    }
    Ok(())
}

fn validate_audit_wire_nonce(event: &ExchangeAuditEvent) -> DurableResult<()> {
    let Some(hash) = event.wire_nonce.strip_prefix("preauth:") else {
        return validate_token("wire nonce", &event.wire_nonce)
            .map_err(|problem| DurableError::InvalidAudit(problem.to_string()));
    };
    validate_hash("pre-authentication nonce digest", hash).map_err(|_| {
        DurableError::InvalidAudit(
            "pre-authentication wire nonce must use preauth:<sha256> format".into(),
        )
    })?;
    if event.direction != AuditDirection::Inbound
        || !matches!(
            event.phase,
            AuditPhase::InboundRequestObserved
                | AuditPhase::InboundDiagnosticReplyWritten
                | AuditPhase::InboundConnectionClosed
        )
    {
        return Err(DurableError::InvalidAudit(
            "pre-authentication wire nonce is limited to inbound observation and diagnostic terminals"
                .into(),
        ));
    }
    if event.authenticated_peer_id.is_some()
        || event.peer_claim.is_some()
        || event.operation_id.is_some()
        || event.local_receipt_operation_id.is_some()
        || event.remote_receipt_operation_id.is_some()
        || event.replayed
    {
        return Err(DurableError::InvalidAudit(
            "pre-authentication wire nonce cannot carry peer, operation, or receipt authority"
                .into(),
        ));
    }
    Ok(())
}

fn validate_audit_semantics(event: &ExchangeAuditEvent) -> DurableResult<()> {
    let is_prepared = matches!(
        event.phase,
        AuditPhase::OutboundRequestPrepared | AuditPhase::InboundReplyPrepared
    );
    if is_prepared != (event.outcome == AuditOutcome::Incomplete) {
        return Err(DurableError::InvalidAudit(
            "prepared phases require incomplete outcome and incomplete is reserved for prepared phases"
                .into(),
        ));
    }
    let has_local_id = event.local_receipt_operation_id.is_some();
    let has_local_hash = event.local_receipt_sha256.is_some();
    let has_remote_id = event.remote_receipt_operation_id.is_some();
    let has_remote_hash = event.remote_receipt_sha256.is_some();
    if has_local_id != has_local_hash || has_remote_id != has_remote_hash {
        return Err(DurableError::InvalidAudit(
            "receipt identity and digest must be supplied together".into(),
        ));
    }
    if event.outcome == AuditOutcome::Accepted && event.authenticated_peer_id.is_none() {
        return Err(DurableError::InvalidAudit(
            "accepted exchange evidence requires an authenticated peer".into(),
        ));
    }
    if matches!(
        event.outcome,
        AuditOutcome::Accepted | AuditOutcome::Incomplete
    ) && (event.error_category.is_some() || event.reason_code.is_some())
    {
        return Err(DurableError::InvalidAudit(
            "successful or prepared evidence cannot carry an error".into(),
        ));
    }
    if event.outcome == AuditOutcome::UnauthenticatedDiagnostic
        && event.authenticated_peer_id.is_some()
    {
        return Err(DurableError::InvalidAudit(
            "unauthenticated diagnostics cannot assert a peer identity".into(),
        ));
    }
    match event.outcome {
        AuditOutcome::AuthenticatedRefusal => {
            validate_authenticated_refusal(event)?;
        }
        AuditOutcome::Unavailable => {
            if event.error_category != Some(AuditErrorCategory::Unavailable)
                || event.reason_code != Some(RefusalReason::TransportUnavailable)
            {
                return Err(DurableError::InvalidAudit(
                    "unavailable evidence requires an unavailable category and transport_unavailable reason"
                        .into(),
                ));
            }
        }
        AuditOutcome::Malformed | AuditOutcome::UnauthenticatedDiagnostic => {
            if event.error_category != Some(AuditErrorCategory::Malformed)
                || event.reason_code != Some(RefusalReason::InvalidRequest)
            {
                return Err(DurableError::InvalidAudit(
                    "malformed or unauthenticated diagnostic evidence requires a malformed category and invalid_request reason"
                        .into(),
                ));
            }
        }
        AuditOutcome::Accepted | AuditOutcome::Incomplete => {}
    }
    if event.replayed
        && event.local_receipt_operation_id.is_none()
        && event.remote_receipt_operation_id.is_none()
    {
        return Err(DurableError::InvalidAudit(
            "replayed evidence requires a receipt reference".into(),
        ));
    }
    if let (Some(claim), Some(peer)) = (&event.peer_claim, &event.authenticated_peer_id) {
        if claim != peer {
            return Err(DurableError::InvalidAudit(
                "authenticated peer claim mismatch".into(),
            ));
        }
    }
    validate_audit_byte_semantics(event)?;
    validate_audit_phase_semantics(event)
}

fn validate_authenticated_refusal(event: &ExchangeAuditEvent) -> DurableResult<()> {
    if event.authenticated_peer_id.is_none()
        || event.error_category != Some(AuditErrorCategory::Refused)
        || event.reason_code.is_none()
    {
        return Err(DurableError::InvalidAudit(
            "authenticated refusal evidence is incomplete".into(),
        ));
    }
    if !matches!(
        event.reason_code,
        Some(
            RefusalReason::InvalidRequest
                | RefusalReason::PolicyViolation
                | RefusalReason::OperationIdReused
        )
    ) {
        return Err(DurableError::InvalidAudit(
            "authenticated refusal reason is not a peer-request refusal".into(),
        ));
    }
    Ok(())
}

fn validate_transfer(
    label: &str,
    frame_bytes: u64,
    announced_body_bytes: Option<u64>,
    sha256: Option<&String>,
    complete: bool,
) -> DurableResult<()> {
    if let Some(body_bytes) = announced_body_bytes {
        if body_bytes > u64::from(u32::MAX) || frame_bytes > body_bytes + 4 {
            return Err(DurableError::InvalidAudit(format!(
                "{label} frame byte accounting is invalid"
            )));
        }
    } else if frame_bytes >= 4 || sha256.is_some() {
        return Err(DurableError::InvalidAudit(format!(
            "{label} frame requires its announced body size"
        )));
    }
    if complete
        && (sha256.is_none() || announced_body_bytes.map(|size| size + 4) != Some(frame_bytes))
    {
        return Err(DurableError::InvalidAudit(format!(
            "complete {label} frame requires digest and exact byte accounting"
        )));
    }
    Ok(())
}

fn require_no_transfer(event: &ExchangeAuditEvent, label: &str) -> DurableResult<()> {
    if event.request_frame_bytes != 0
        || event.request_announced_body_bytes.is_some()
        || event.request_sha256.is_some()
        || event.reply_frame_bytes != 0
        || event.reply_announced_body_bytes.is_some()
        || event.reply_sha256.is_some()
    {
        return Err(DurableError::InvalidAudit(format!(
            "{label} phase cannot carry transferred bytes or frame intent"
        )));
    }
    Ok(())
}

fn validate_audit_byte_semantics(event: &ExchangeAuditEvent) -> DurableResult<()> {
    match event.direction {
        AuditDirection::Outbound => validate_outbound_audit_bytes(event),
        AuditDirection::Inbound => validate_inbound_audit_bytes(event),
    }
}

fn validate_outbound_audit_bytes(event: &ExchangeAuditEvent) -> DurableResult<()> {
    match event.phase {
        AuditPhase::OutboundRequestPrepared => {
            if event.request_frame_bytes != 0
                || event.reply_frame_bytes != 0
                || event.reply_announced_body_bytes.is_some()
                || event.reply_sha256.is_some()
            {
                return Err(DurableError::InvalidAudit(
                    "outbound request preparation records intent with zero transferred bytes"
                        .into(),
                ));
            }
            validate_transfer(
                "prepared outbound request",
                0,
                event.request_announced_body_bytes,
                event.request_sha256.as_ref(),
                false,
            )?;
            if event.request_announced_body_bytes.is_none() || event.request_sha256.is_none() {
                return Err(DurableError::InvalidAudit(
                    "outbound request preparation requires bounded request intent".into(),
                ));
            }
        }
        AuditPhase::OutboundExchangeCompleted => {
            let complete_request = matches!(
                event.outcome,
                AuditOutcome::Accepted
                    | AuditOutcome::AuthenticatedRefusal
                    | AuditOutcome::UnauthenticatedDiagnostic
                    | AuditOutcome::Malformed
            );
            validate_transfer(
                "outbound request",
                event.request_frame_bytes,
                event.request_announced_body_bytes,
                event.request_sha256.as_ref(),
                complete_request,
            )?;
            validate_transfer(
                "inbound reply",
                event.reply_frame_bytes,
                event.reply_announced_body_bytes,
                event.reply_sha256.as_ref(),
                matches!(
                    event.outcome,
                    AuditOutcome::Accepted | AuditOutcome::AuthenticatedRefusal
                ),
            )?;
        }
        _ => {
            return Err(DurableError::InvalidAudit(
                "inbound audit phase cannot use outbound direction".into(),
            ));
        }
    }
    Ok(())
}

fn validate_inbound_audit_bytes(event: &ExchangeAuditEvent) -> DurableResult<()> {
    match event.phase {
        AuditPhase::InboundRequestObserved => {
            validate_transfer(
                "inbound request",
                event.request_frame_bytes,
                event.request_announced_body_bytes,
                event.request_sha256.as_ref(),
                event.outcome == AuditOutcome::Accepted || event.request_sha256.is_some(),
            )?;
            if event.reply_frame_bytes != 0
                || event.reply_announced_body_bytes.is_some()
                || event.reply_sha256.is_some()
            {
                return Err(DurableError::InvalidAudit(
                    "inbound request observation cannot carry reply transfer evidence".into(),
                ));
            }
        }
        AuditPhase::InboundImportCommitted => require_no_transfer(event, "inbound import")?,
        AuditPhase::InboundRefusalRecorded => require_no_transfer(event, "inbound refusal")?,
        AuditPhase::InboundReplyPrepared => {
            if event.request_frame_bytes != 0
                || event.request_announced_body_bytes.is_some()
                || event.request_sha256.is_some()
                || event.reply_frame_bytes != 0
            {
                return Err(DurableError::InvalidAudit(
                    "inbound reply preparation records intent with zero transferred bytes".into(),
                ));
            }
            validate_transfer(
                "prepared inbound reply",
                0,
                event.reply_announced_body_bytes,
                event.reply_sha256.as_ref(),
                false,
            )?;
            if event.reply_announced_body_bytes.is_none() || event.reply_sha256.is_none() {
                return Err(DurableError::InvalidAudit(
                    "inbound reply preparation requires bounded reply intent".into(),
                ));
            }
        }
        AuditPhase::InboundReplyWriteObserved => {
            if event.request_frame_bytes != 0
                || event.request_announced_body_bytes.is_some()
                || event.request_sha256.is_some()
            {
                return Err(DurableError::InvalidAudit(
                    "inbound reply write cannot repeat request transfer evidence".into(),
                ));
            }
            validate_transfer(
                "outbound reply",
                event.reply_frame_bytes,
                event.reply_announced_body_bytes,
                event.reply_sha256.as_ref(),
                matches!(
                    event.outcome,
                    AuditOutcome::Accepted | AuditOutcome::AuthenticatedRefusal
                ),
            )?;
            if event.outcome == AuditOutcome::Unavailable {
                let complete = event.reply_announced_body_bytes.map(|size| size + 4);
                if event.reply_frame_bytes == 0 || complete == Some(event.reply_frame_bytes) {
                    return Err(DurableError::InvalidAudit(
                        "unavailable reply write requires a nonzero partial frame".into(),
                    ));
                }
            }
        }
        AuditPhase::InboundDiagnosticReplyWritten => {
            validate_inbound_diagnostic_bytes(event)?;
        }
        AuditPhase::InboundConnectionClosed => {
            require_no_transfer(event, "inbound connection close")?;
        }
        _ => {
            return Err(DurableError::InvalidAudit(
                "outbound audit phase cannot use inbound direction".into(),
            ));
        }
    }
    Ok(())
}

fn validate_inbound_diagnostic_bytes(event: &ExchangeAuditEvent) -> DurableResult<()> {
    if event.request_frame_bytes != 0
        || event.request_announced_body_bytes.is_some()
        || event.request_sha256.is_some()
    {
        return Err(DurableError::InvalidAudit(
            "inbound diagnostic reply cannot repeat request transfer evidence".into(),
        ));
    }
    validate_transfer(
        "outbound diagnostic reply",
        event.reply_frame_bytes,
        event.reply_announced_body_bytes,
        event.reply_sha256.as_ref(),
        false,
    )?;
    let complete = event.reply_announced_body_bytes.map(|size| size + 4);
    if event.reply_frame_bytes == 0
        || event.reply_announced_body_bytes.is_none()
        || event.reply_sha256.is_none()
    {
        return Err(DurableError::InvalidAudit(
            "diagnostic reply write requires bounded nonzero reply evidence".into(),
        ));
    }
    if complete != Some(event.reply_frame_bytes)
        && (event.outcome != AuditOutcome::Unavailable
            || event.error_category != Some(AuditErrorCategory::Unavailable))
    {
        return Err(DurableError::InvalidAudit(
            "partial diagnostic reply write must record unavailability".into(),
        ));
    }
    Ok(())
}

fn validate_audit_phase_semantics(event: &ExchangeAuditEvent) -> DurableResult<()> {
    let valid_outcome = match event.phase {
        AuditPhase::OutboundRequestPrepared | AuditPhase::InboundReplyPrepared => {
            event.outcome == AuditOutcome::Incomplete
        }
        AuditPhase::OutboundExchangeCompleted => event.outcome != AuditOutcome::Incomplete,
        AuditPhase::InboundRequestObserved => matches!(
            event.outcome,
            AuditOutcome::Accepted
                | AuditOutcome::UnauthenticatedDiagnostic
                | AuditOutcome::Unavailable
                | AuditOutcome::Malformed
        ),
        AuditPhase::InboundImportCommitted => event.outcome == AuditOutcome::Accepted,
        AuditPhase::InboundRefusalRecorded => event.outcome == AuditOutcome::AuthenticatedRefusal,
        AuditPhase::InboundReplyWriteObserved => matches!(
            event.outcome,
            AuditOutcome::Accepted | AuditOutcome::AuthenticatedRefusal | AuditOutcome::Unavailable
        ),
        AuditPhase::InboundDiagnosticReplyWritten => matches!(
            event.outcome,
            AuditOutcome::UnauthenticatedDiagnostic
                | AuditOutcome::Unavailable
                | AuditOutcome::Malformed
        ),
        AuditPhase::InboundConnectionClosed => matches!(
            event.outcome,
            AuditOutcome::UnauthenticatedDiagnostic
                | AuditOutcome::Unavailable
                | AuditOutcome::Malformed
        ),
    };
    if !valid_outcome {
        return Err(DurableError::InvalidAudit(
            "audit outcome is invalid for its phase".into(),
        ));
    }
    if event.phase == AuditPhase::OutboundExchangeCompleted
        && event.outcome == AuditOutcome::Accepted
        && event.remote_receipt_operation_id.is_none()
    {
        return Err(DurableError::InvalidAudit(
            "accepted outbound completion requires remote receipt evidence".into(),
        ));
    }
    if event.phase == AuditPhase::InboundImportCommitted
        && (event.direction != AuditDirection::Inbound
            || event.local_receipt_operation_id.is_none())
    {
        return Err(DurableError::InvalidAudit(
            "inbound import evidence must reference its accepted local receipt".into(),
        ));
    }
    if matches!(
        event.phase,
        AuditPhase::OutboundRequestPrepared | AuditPhase::OutboundExchangeCompleted
    ) && event.direction != AuditDirection::Outbound
    {
        return Err(DurableError::InvalidAudit(
            "outbound audit phase requires outbound direction".into(),
        ));
    }
    if matches!(
        event.phase,
        AuditPhase::InboundRequestObserved
            | AuditPhase::InboundImportCommitted
            | AuditPhase::InboundRefusalRecorded
            | AuditPhase::InboundReplyPrepared
            | AuditPhase::InboundReplyWriteObserved
            | AuditPhase::InboundDiagnosticReplyWritten
            | AuditPhase::InboundConnectionClosed
    ) && event.direction != AuditDirection::Inbound
    {
        return Err(DurableError::InvalidAudit(
            "inbound audit phase requires inbound direction".into(),
        ));
    }
    if event.remote_receipt_operation_id.is_some()
        && (event.phase != AuditPhase::OutboundExchangeCompleted
            || event.outcome != AuditOutcome::Accepted)
    {
        return Err(DurableError::InvalidAudit(
            "only an accepted outbound completion can reference a remote receipt".into(),
        ));
    }
    Ok(())
}

fn validate_audit_identity(
    topology: &Topology,
    replica_id: &str,
    event: &ExchangeAuditEvent,
) -> DurableResult<()> {
    if event.phase == AuditPhase::OutboundExchangeCompleted
        && event.outcome == AuditOutcome::Accepted
    {
        let wire_operation_id = event.operation_id.as_deref().ok_or_else(|| {
            DurableError::InvalidAudit(
                "accepted outbound completion requires a wire operation ID".into(),
            )
        })?;
        let expected = authenticated_import_receipt_id_parts(
            topology.logical_manager_id(),
            replica_id,
            wire_operation_id,
        )?;
        if event.remote_receipt_operation_id.as_deref() != Some(expected.as_str()) {
            return Err(DurableError::InvalidAudit(
                "remote receipt identity does not match the authenticated import mapping".into(),
            ));
        }
    }
    Ok(())
}

fn audit_digest(record_json: &str) -> DurableResult<String> {
    Ok(digest(&json(&(
        "podmesh-manager-ha-exchange-audit/1",
        record_json,
    ))?))
}

fn enum_text(value: &impl Serialize) -> DurableResult<String> {
    match serde_json::to_value(value).map_err(error)? {
        serde_json::Value::String(text) => Ok(text),
        _ => Err(DurableError::Storage(
            "audit enum did not serialize as text".into(),
        )),
    }
}

fn enum_from_text<T: for<'de> Deserialize<'de>>(value: &str) -> DurableResult<T> {
    serde_json::from_value(serde_json::Value::String(value.into())).map_err(error)
}

fn audit_enum_from_text<T: for<'de> Deserialize<'de>>(value: &str) -> DurableResult<T> {
    enum_from_text(value).map_err(|_| DurableError::Corrupt("stored audit enum is invalid".into()))
}

fn receipt_metadata(
    connection: &Connection,
    operation_id: &str,
    sha256: &str,
) -> DurableResult<Option<ReceiptMetadata>> {
    connection
        .query_row(
            "SELECT kind, source_replica_id, wire_operation_id FROM receipts WHERE operation_id=?1 AND sha256=?2",
            params![operation_id, sha256],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(error)?
        .map(|(kind, source_replica_id, wire_operation_id)| {
            Ok(ReceiptMetadata {
                kind: enum_from_text(&kind)?,
                source_replica_id,
                wire_operation_id,
            })
        })
        .transpose()
}

fn verify_audit_receipt_link(
    connection: &Connection,
    event: &ExchangeAuditEvent,
) -> DurableResult<()> {
    let Some((operation_id, sha256)) = local_receipt_reference(event) else {
        return Ok(());
    };
    check_audit_receipt_link(event, receipt_metadata(connection, operation_id, sha256)?)
}

/// The local receipt an audit event references: its identity and digest.
fn local_receipt_reference(event: &ExchangeAuditEvent) -> Option<(&str, &str)> {
    event
        .local_receipt_operation_id
        .as_deref()
        .zip(event.local_receipt_sha256.as_deref())
}

/// Checks an audit event's local receipt reference against the metadata of the
/// stored receipt with that identity and digest, `None` when there is none.
fn check_audit_receipt_link(
    event: &ExchangeAuditEvent,
    metadata: Option<ReceiptMetadata>,
) -> DurableResult<()> {
    let Some(metadata) = metadata else {
        return Err(DurableError::InvalidAudit(
            "local receipt reference is missing or has a mismatched digest".into(),
        ));
    };
    if matches!(
        event.phase,
        AuditPhase::InboundImportCommitted
            | AuditPhase::InboundReplyPrepared
            | AuditPhase::InboundReplyWriteObserved
    ) {
        if metadata.kind != ReceiptKind::AuthenticatedImport {
            return Err(DurableError::InvalidAudit(
                "local audit receipt must be an authenticated import".into(),
            ));
        }
        if metadata.source_replica_id != event.authenticated_peer_id {
            return Err(DurableError::InvalidAudit(
                "local audit receipt source does not match the authenticated peer".into(),
            ));
        }
        if metadata.wire_operation_id != event.operation_id {
            return Err(DurableError::InvalidAudit(
                "local import receipt wire operation does not match audit".into(),
            ));
        }
    } else {
        return Err(DurableError::InvalidAudit(
            "this audit phase cannot reference a local receipt".into(),
        ));
    }
    Ok(())
}

/// Checks a candidate against the stored rows of its own attempt only, as read
/// and verified by [`load_attempt_audits`]. Every sequence rule is scoped to one
/// attempt ID (per direction, plus the rule that an attempt ID cannot span both
/// directions), so over a verified store this gives the same result as checking
/// the candidate against the whole table.
fn validate_candidate_audit_sequence(
    attempt: &[ExchangeAuditEvidence],
    event: &ExchangeAuditEvent,
) -> DurableResult<()> {
    let mut events: Vec<_> = attempt.iter().map(|entry| &entry.event).collect();
    events.push(event);
    audit_sequence_problem(events).map_or(Ok(()), |detail| Err(DurableError::InvalidAudit(detail)))
}

/// The checks of a candidate audit event that read no stored row: its fields and
/// semantics, its receipt identities and its authenticated peer, which must be
/// another configured replica.
fn validate_candidate_audit(
    topology: &Topology,
    replica_id: &str,
    event: &ExchangeAuditEvent,
) -> DurableResult<()> {
    validate_audit(event)?;
    validate_audit_identity(topology, replica_id, event)?;
    if let Some(peer_id) = event.authenticated_peer_id.as_deref() {
        topology.instantiate(peer_id).map_err(|_| {
            DurableError::InvalidAudit("authenticated peer is not configured".into())
        })?;
        if peer_id == replica_id {
            return Err(DurableError::InvalidAudit(
                "authenticated peer must be a distinct configured replica".into(),
            ));
        }
    }
    Ok(())
}

fn prevalidate_authenticated_import_audit(
    transaction: &Transaction<'_>,
    integrity: &StoreIntegrityEntry,
    topology: &Topology,
    replica_id: &str,
    audit: &ExchangeAuditEvent,
    local_operation_id: &str,
) -> DurableResult<()> {
    let mut candidate = audit.clone();
    candidate.local_receipt_operation_id = Some(local_operation_id.into());
    candidate.local_receipt_sha256 = Some(format!("{:064x}", 0));
    validate_candidate_audit(topology, replica_id, &candidate)?;
    let existing = integrity.read_at_use(load_audit_by_id(
        transaction,
        topology,
        replica_id,
        &candidate.audit_event_id,
    ))?;
    if let Some(existing) = existing {
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
        let attempt = integrity.read_at_use(load_attempt_audits(
            transaction,
            topology,
            replica_id,
            &candidate.attempt_id,
        ))?;
        validate_candidate_audit_sequence(&attempt, &candidate)?;
    }
    Ok(())
}

fn same_attempt_metadata(first: &ExchangeAuditEvent, next: &ExchangeAuditEvent) -> bool {
    first.wire_nonce == next.wire_nonce && first.operation_id == next.operation_id
}

fn same_authenticated_peer(first: &ExchangeAuditEvent, next: &ExchangeAuditEvent) -> bool {
    first.authenticated_peer_id.is_some()
        && first.authenticated_peer_id == next.authenticated_peer_id
}

fn same_peer_reference(first: &ExchangeAuditEvent, next: &ExchangeAuditEvent) -> bool {
    first
        .authenticated_peer_id
        .as_ref()
        .or(first.peer_claim.as_ref())
        == next
            .authenticated_peer_id
            .as_ref()
            .or(next.peer_claim.as_ref())
}

fn same_local_receipt(first: &ExchangeAuditEvent, next: &ExchangeAuditEvent) -> bool {
    first.local_receipt_operation_id == next.local_receipt_operation_id
        && first.local_receipt_sha256 == next.local_receipt_sha256
}

fn audit_sequence_problem<'a>(
    events: impl IntoIterator<Item = &'a ExchangeAuditEvent>,
) -> Option<String> {
    let mut directions = BTreeMap::new();
    let mut attempts: BTreeMap<(AuditDirection, &str), BTreeMap<AuditPhase, &ExchangeAuditEvent>> =
        BTreeMap::new();
    for event in events {
        if let Some(direction) = directions.insert(event.attempt_id.as_str(), event.direction) {
            if direction != event.direction {
                return Some("one attempt ID cannot span both audit directions".into());
            }
        }
        if attempts
            .entry((event.direction, event.attempt_id.as_str()))
            .or_default()
            .insert(event.phase, event)
            .is_some()
        {
            return Some("exchange attempt already contains this phase".into());
        }
    }
    for ((direction, _), phases) in attempts {
        let problem = match direction {
            AuditDirection::Outbound => validate_outbound_sequence(&phases),
            AuditDirection::Inbound => validate_inbound_sequence(&phases),
        };
        if problem.is_some() {
            return problem;
        }
    }
    None
}

fn validate_outbound_sequence(
    phases: &BTreeMap<AuditPhase, &ExchangeAuditEvent>,
) -> Option<String> {
    if phases.keys().any(|phase| {
        !matches!(
            phase,
            AuditPhase::OutboundRequestPrepared | AuditPhase::OutboundExchangeCompleted
        )
    }) {
        return Some("outbound attempt contains an inbound phase".into());
    }
    let Some(prepared) = phases.get(&AuditPhase::OutboundRequestPrepared) else {
        return Some("outbound exchange lacks its prepared predecessor".into());
    };
    if let Some(completed) = phases.get(&AuditPhase::OutboundExchangeCompleted) {
        if !same_attempt_metadata(prepared, completed) || !same_peer_reference(prepared, completed)
        {
            return Some("outbound exchange phases do not describe one attempt".into());
        }
        if prepared.request_announced_body_bytes != completed.request_announced_body_bytes
            || prepared.request_sha256 != completed.request_sha256
        {
            return Some("outbound completion does not match its prepared request".into());
        }
    }
    None
}

fn validate_inbound_sequence(phases: &BTreeMap<AuditPhase, &ExchangeAuditEvent>) -> Option<String> {
    if phases.keys().any(|phase| {
        matches!(
            phase,
            AuditPhase::OutboundRequestPrepared | AuditPhase::OutboundExchangeCompleted
        )
    }) {
        return Some("inbound attempt contains an outbound phase".into());
    }
    let Some(observed) = phases.get(&AuditPhase::InboundRequestObserved) else {
        return Some("inbound exchange lacks its request observation".into());
    };
    if phases
        .values()
        .any(|event| !same_attempt_metadata(observed, event))
    {
        return Some("inbound exchange phases do not describe one attempt".into());
    }
    let imported = phases.get(&AuditPhase::InboundImportCommitted).copied();
    let refused = phases.get(&AuditPhase::InboundRefusalRecorded).copied();
    if imported.is_some() && refused.is_some() {
        return Some("inbound exchange contains both accepted and refused decisions".into());
    }
    let decision = imported.or(refused);
    if let Some(decision) = decision {
        if observed.outcome != AuditOutcome::Accepted
            || !same_authenticated_peer(observed, decision)
        {
            return Some(
                "authenticated inbound decision requires an authenticated accepted observation"
                    .into(),
            );
        }
    }
    let prepared = phases.get(&AuditPhase::InboundReplyPrepared).copied();
    if let Some(prepared) = prepared {
        let Some(decision) = decision else {
            return Some("inbound reply preparation lacks a durable decision".into());
        };
        if !same_authenticated_peer(decision, prepared) || !same_local_receipt(decision, prepared) {
            return Some("inbound reply preparation does not match its durable decision".into());
        }
    }
    if let Some(written) = phases.get(&AuditPhase::InboundReplyWriteObserved) {
        let Some(prepared) = prepared else {
            return Some("inbound reply write lacks its prepared predecessor".into());
        };
        let decision = decision.expect("prepared reply has a decision");
        if !same_authenticated_peer(prepared, written) || !same_local_receipt(decision, written) {
            return Some("inbound reply write does not match its prepared decision".into());
        }
        if prepared.reply_announced_body_bytes != written.reply_announced_body_bytes
            || prepared.reply_sha256 != written.reply_sha256
        {
            return Some("inbound reply write does not match its prepared frame".into());
        }
        let decision_matches = match decision.phase {
            AuditPhase::InboundImportCommitted => written.outcome == AuditOutcome::Accepted,
            AuditPhase::InboundRefusalRecorded => {
                written.outcome == AuditOutcome::AuthenticatedRefusal
                    && written.error_category == decision.error_category
                    && written.reason_code == decision.reason_code
            }
            _ => false,
        };
        if written.outcome != AuditOutcome::Unavailable && !decision_matches {
            return Some("inbound reply outcome does not match its durable decision".into());
        }
    }
    if let Some(problem) = validate_inbound_diagnostic(phases, observed, decision, prepared) {
        return Some(problem);
    }
    if let Some(problem) = validate_inbound_close(phases, observed, decision, prepared) {
        return Some(problem);
    }
    None
}

fn validate_inbound_diagnostic(
    phases: &BTreeMap<AuditPhase, &ExchangeAuditEvent>,
    observed: &ExchangeAuditEvent,
    decision: Option<&ExchangeAuditEvent>,
    prepared: Option<&ExchangeAuditEvent>,
) -> Option<String> {
    let diagnostic = phases.get(&AuditPhase::InboundDiagnosticReplyWritten)?;
    if decision.is_some()
        || prepared.is_some()
        || phases.contains_key(&AuditPhase::InboundReplyWriteObserved)
    {
        return Some("diagnostic reply cannot follow a durable or prepared signed decision".into());
    }
    if phases.contains_key(&AuditPhase::InboundConnectionClosed) {
        return Some("an inbound attempt cannot both write a diagnostic and close".into());
    }
    let complete = diagnostic.reply_announced_body_bytes.map(|size| size + 4)
        == Some(diagnostic.reply_frame_bytes);
    if observed.outcome == AuditOutcome::Accepted {
        if diagnostic.outcome != AuditOutcome::Unavailable
            || !same_authenticated_peer(observed, diagnostic)
        {
            return Some(
                "diagnostic after authenticated observation must record matching unavailability"
                    .into(),
            );
        }
    } else {
        if diagnostic.authenticated_peer_id.is_some() {
            return Some(
                "diagnostic after unauthenticated observation cannot assert an authenticated peer"
                    .into(),
            );
        }
        if !same_peer_reference(observed, diagnostic) {
            return Some("diagnostic reply does not match its observed diagnostic".into());
        }
        if complete
            && (diagnostic.outcome != observed.outcome
                || diagnostic.error_category != observed.error_category
                || diagnostic.reason_code != observed.reason_code)
        {
            return Some("diagnostic reply does not match its observed diagnostic".into());
        }
    }
    None
}

fn validate_inbound_close(
    phases: &BTreeMap<AuditPhase, &ExchangeAuditEvent>,
    observed: &ExchangeAuditEvent,
    decision: Option<&ExchangeAuditEvent>,
    prepared: Option<&ExchangeAuditEvent>,
) -> Option<String> {
    let closed = phases.get(&AuditPhase::InboundConnectionClosed)?;
    if phases.contains_key(&AuditPhase::InboundDiagnosticReplyWritten) {
        return Some("an inbound attempt cannot both write a diagnostic and close".into());
    }
    if phases.contains_key(&AuditPhase::InboundReplyWriteObserved) {
        return Some("an inbound attempt cannot both write a reply and close without one".into());
    }
    if let Some(prepared) = prepared {
        if closed.outcome != AuditOutcome::Unavailable
            || !same_attempt_metadata(prepared, closed)
            || !same_authenticated_peer(prepared, closed)
        {
            return Some(
                "close after reply preparation must record matching unavailability".into(),
            );
        }
        return None;
    }
    if decision.is_some() {
        return Some("a durable inbound decision must prepare a reply before close".into());
    }
    if observed.outcome == AuditOutcome::Accepted {
        if closed.outcome != AuditOutcome::Unavailable || !same_authenticated_peer(observed, closed)
        {
            return Some(
                "close after authenticated observation must record matching unavailability".into(),
            );
        }
    } else if closed.outcome != observed.outcome
        || closed.error_category != observed.error_category
        || closed.reason_code != observed.reason_code
        || !same_peer_reference(observed, closed)
    {
        return Some("unauthenticated close does not match its observed diagnostic".into());
    }
    None
}

fn insert_audit(
    transaction: &Transaction<'_>,
    integrity: &StoreIntegrityEntry,
    topology: &Topology,
    replica_id: &str,
    event: &ExchangeAuditEvent,
    allow_atomic_import_replay_normalization: bool,
) -> DurableResult<ExchangeAuditEvidence> {
    validate_candidate_audit(topology, replica_id, event)?;
    let record_json = json(event)?;
    let sha256 = audit_digest(&record_json)?;
    // A replay returns stored evidence, so that row is verified before comparison;
    // its canonical record JSON then equals the stored one.
    let prior = integrity.read_at_use(load_audit_by_id(
        transaction,
        topology,
        replica_id,
        &event.audit_event_id,
    ))?;
    if let Some(evidence) = replay_existing_audit(
        prior,
        event,
        &record_json,
        &sha256,
        allow_atomic_import_replay_normalization,
    )? {
        return Ok(evidence);
    }
    let attempt = integrity.read_at_use(load_attempt_audits(
        transaction,
        topology,
        replica_id,
        &event.attempt_id,
    ))?;
    validate_candidate_audit_sequence(&attempt, event)?;
    if let Some((operation_id, receipt_sha256)) = local_receipt_reference(event) {
        let receipt =
            integrity.read_at_use(receipt_metadata(transaction, operation_id, receipt_sha256))?;
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
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
             )",
            params![
                event.audit_event_id,
                event.attempt_id,
                event.wire_nonce,
                enum_text(&event.direction)?,
                enum_text(&event.phase)?,
                event.authenticated_peer_id,
                event.peer_claim,
                event.operation_id,
                i64::try_from(event.request_frame_bytes).map_err(error)?,
                event
                    .request_announced_body_bytes
                    .map(i64::try_from)
                    .transpose()
                    .map_err(error)?,
                event.request_sha256,
                i64::try_from(event.reply_frame_bytes).map_err(error)?,
                event
                    .reply_announced_body_bytes
                    .map(i64::try_from)
                    .transpose()
                    .map_err(error)?,
                event.reply_sha256,
                enum_text(&event.outcome)?,
                event
                    .error_category
                    .map(|value| enum_text(&value))
                    .transpose()?,
                event
                    .reason_code
                    .map(|value| enum_text(&value))
                    .transpose()?,
                event.local_receipt_operation_id,
                event.local_receipt_sha256,
                event.remote_receipt_operation_id,
                event.remote_receipt_sha256,
                i64::from(event.replayed),
                record_json,
                sha256,
            ],
        )
        .map_err(error)?;
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

/// Compares a candidate audit event with the verified stored row of the same
/// audit event ID, if there is one, and returns the evidence of an identical
/// replay.
fn replay_existing_audit(
    prior: Option<ExchangeAuditEvidence>,
    event: &ExchangeAuditEvent,
    record_json: &str,
    sha256: &str,
    allow_atomic_import_replay_normalization: bool,
) -> DurableResult<Option<ExchangeAuditEvidence>> {
    let prior = prior
        .map(|evidence| Ok::<_, DurableError>((json(&evidence.event)?, evidence.sha256)))
        .transpose()?;
    if let Some((prior_json, prior_sha256)) = prior {
        if prior_json != record_json || prior_sha256 != sha256 {
            if !allow_atomic_import_replay_normalization {
                return Err(DurableError::InvalidAudit(
                    "audit event ID reused with different bytes".into(),
                ));
            }
            let mut prior_event: ExchangeAuditEvent = serde_json::from_str(&prior_json)
                .map_err(|_| DurableError::Corrupt("stored audit record is invalid".into()))?;
            prior_event.replayed = event.replayed;
            if prior_event != *event || audit_digest(&prior_json)? != prior_sha256 {
                return Err(DurableError::InvalidAudit(
                    "audit event ID reused with different bytes".into(),
                ));
            }
            return Ok(Some(ExchangeAuditEvidence {
                event: serde_json::from_str(&prior_json)
                    .map_err(|_| DurableError::Corrupt("stored audit record is invalid".into()))?,
                sha256: prior_sha256,
            }));
        }
        return Ok(Some(ExchangeAuditEvidence {
            event: event.clone(),
            sha256: sha256.into(),
        }));
    }
    Ok(None)
}

struct StoredAuditRow {
    audit_event_id: String,
    attempt_id: String,
    wire_nonce: String,
    direction: String,
    phase: String,
    authenticated_peer_id: Option<String>,
    peer_claim: Option<String>,
    operation_id: Option<String>,
    request_frame_bytes: i64,
    request_announced_body_bytes: Option<i64>,
    request_sha256: Option<String>,
    reply_frame_bytes: i64,
    reply_announced_body_bytes: Option<i64>,
    reply_sha256: Option<String>,
    outcome: String,
    error_category: Option<String>,
    reason_code: Option<String>,
    local_receipt_operation_id: Option<String>,
    local_receipt_sha256: Option<String>,
    remote_receipt_operation_id: Option<String>,
    remote_receipt_sha256: Option<String>,
    replayed: i64,
    record_json: String,
    sha256: String,
}

fn read_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredAuditRow> {
    Ok(StoredAuditRow {
        audit_event_id: row.get(0)?,
        attempt_id: row.get(1)?,
        wire_nonce: row.get(2)?,
        direction: row.get(3)?,
        phase: row.get(4)?,
        authenticated_peer_id: row.get(5)?,
        peer_claim: row.get(6)?,
        operation_id: row.get(7)?,
        request_frame_bytes: row.get(8)?,
        request_announced_body_bytes: row.get(9)?,
        request_sha256: row.get(10)?,
        reply_frame_bytes: row.get(11)?,
        reply_announced_body_bytes: row.get(12)?,
        reply_sha256: row.get(13)?,
        outcome: row.get(14)?,
        error_category: row.get(15)?,
        reason_code: row.get(16)?,
        local_receipt_operation_id: row.get(17)?,
        local_receipt_sha256: row.get(18)?,
        remote_receipt_operation_id: row.get(19)?,
        remote_receipt_sha256: row.get(20)?,
        replayed: row.get(21)?,
        record_json: row.get(22)?,
        sha256: row.get(23)?,
    })
}

fn decode_audit_row(row: StoredAuditRow) -> DurableResult<(ExchangeAuditEvent, String, String)> {
    let event = ExchangeAuditEvent {
        audit_event_id: row.audit_event_id,
        attempt_id: row.attempt_id,
        wire_nonce: row.wire_nonce,
        direction: audit_enum_from_text(&row.direction)?,
        phase: audit_enum_from_text(&row.phase)?,
        authenticated_peer_id: row.authenticated_peer_id,
        peer_claim: row.peer_claim,
        operation_id: row.operation_id,
        request_frame_bytes: u64::try_from(row.request_frame_bytes)
            .map_err(|_| DurableError::Corrupt("stored audit byte count is negative".into()))?,
        request_announced_body_bytes: row
            .request_announced_body_bytes
            .map(u64::try_from)
            .transpose()
            .map_err(|_| DurableError::Corrupt("stored audit byte count is negative".into()))?,
        request_sha256: row.request_sha256,
        reply_frame_bytes: u64::try_from(row.reply_frame_bytes)
            .map_err(|_| DurableError::Corrupt("stored audit byte count is negative".into()))?,
        reply_announced_body_bytes: row
            .reply_announced_body_bytes
            .map(u64::try_from)
            .transpose()
            .map_err(|_| DurableError::Corrupt("stored audit byte count is negative".into()))?,
        reply_sha256: row.reply_sha256,
        outcome: audit_enum_from_text(&row.outcome)?,
        error_category: row
            .error_category
            .map(|value| audit_enum_from_text(&value))
            .transpose()?,
        reason_code: row
            .reason_code
            .map(|value| audit_enum_from_text(&value))
            .transpose()?,
        local_receipt_operation_id: row.local_receipt_operation_id,
        local_receipt_sha256: row.local_receipt_sha256,
        remote_receipt_operation_id: row.remote_receipt_operation_id,
        remote_receipt_sha256: row.remote_receipt_sha256,
        replayed: match row.replayed {
            0 => false,
            1 => true,
            _ => {
                return Err(DurableError::Corrupt(
                    "stored audit replay flag is invalid".into(),
                ));
            }
        },
    };
    Ok((event, row.record_json, row.sha256))
}

fn verify_loaded_audit(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    event: ExchangeAuditEvent,
    record_json: &str,
    sha256: String,
) -> DurableResult<ExchangeAuditEvidence> {
    validate_audit(&event)
        .map_err(|problem| DurableError::Corrupt(format!("stored audit is invalid: {problem}")))?;
    validate_audit_identity(topology, replica_id, &event).map_err(|problem| {
        DurableError::Corrupt(format!("stored audit identity is invalid: {problem}"))
    })?;
    if let Some(peer_id) = event.authenticated_peer_id.as_deref() {
        topology.instantiate(peer_id).map_err(|_| {
            DurableError::Corrupt("stored audit authenticates an unknown peer".into())
        })?;
        if peer_id == replica_id {
            return Err(DurableError::Corrupt(
                "stored audit authenticates the local replica as its peer".into(),
            ));
        }
    }
    if json(&event)? != record_json || audit_digest(record_json)? != sha256 {
        return Err(DurableError::Corrupt(
            "stored audit record or hash mismatch".into(),
        ));
    }
    verify_audit_receipt_link(connection, &event).map_err(|problem| {
        DurableError::Corrupt(format!("stored audit receipt link is invalid: {problem}"))
    })?;
    Ok(ExchangeAuditEvidence { event, sha256 })
}

const AUDIT_COLUMNS: &str = "audit_event_id, attempt_id, wire_nonce, direction, phase,
    authenticated_peer_id, peer_claim, operation_id, request_frame_bytes,
    request_announced_body_bytes, request_sha256, reply_frame_bytes,
    reply_announced_body_bytes, reply_sha256, outcome, error_category, reason_code,
    local_receipt_operation_id, local_receipt_sha256,
    remote_receipt_operation_id, remote_receipt_sha256, replayed,
    record_json, sha256";

fn load_audits(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<Vec<ExchangeAuditEvidence>> {
    let evidence = query_verified_audits(
        connection,
        topology,
        replica_id,
        &format!("SELECT {AUDIT_COLUMNS} FROM exchange_audit_events ORDER BY audit_event_id"),
        [],
    )?;
    verify_audit_sequences(&evidence)?;
    Ok(evidence)
}

/// Loads and verifies the stored rows of one attempt ID in both directions,
/// through the `UNIQUE(direction, attempt_id, phase)` index.
fn load_attempt_audits(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    attempt_id: &str,
) -> DurableResult<Vec<ExchangeAuditEvidence>> {
    let evidence = query_verified_audits(
        connection,
        topology,
        replica_id,
        &format!(
            "SELECT {AUDIT_COLUMNS} FROM exchange_audit_events
             WHERE direction IN ('inbound', 'outbound') AND attempt_id=?1
             ORDER BY audit_event_id"
        ),
        [attempt_id],
    )?;
    verify_audit_sequences(&evidence)?;
    Ok(evidence)
}

fn load_audit_by_id(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    audit_event_id: &str,
) -> DurableResult<Option<ExchangeAuditEvidence>> {
    Ok(query_verified_audits(
        connection,
        topology,
        replica_id,
        &format!("SELECT {AUDIT_COLUMNS} FROM exchange_audit_events WHERE audit_event_id=?1"),
        [audit_event_id],
    )?
    .pop())
}

fn query_verified_audits(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> DurableResult<Vec<ExchangeAuditEvidence>> {
    let mut statement = connection.prepare(sql).map_err(error)?;
    let rows = statement
        .query_map(parameters, read_audit_row)
        .map_err(error)?;
    let mut evidence = Vec::new();
    for row in rows {
        let (event, record_json, sha256) = decode_audit_row(row.map_err(error)?)?;
        evidence.push(verify_loaded_audit(
            connection,
            topology,
            replica_id,
            event,
            &record_json,
            sha256,
        )?);
    }
    Ok(evidence)
}

/// Verifies audit rows appended after `from` one by one, then the complete
/// stored sequence of every attempt they belong to.
fn verify_appended_audits(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
    from: &TablePosition,
) -> DurableResult<TablePosition> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {AUDIT_COLUMNS}, rowid FROM exchange_audit_events WHERE rowid > ?1 ORDER BY rowid"
        ))
        .map_err(error)?;
    let rows = statement
        .query_map([from.rowid], |row| {
            Ok((read_audit_row(row)?, row.get::<_, i64>(24)?))
        })
        .map_err(error)?;
    let mut position = from.clone();
    let mut attempts = BTreeSet::new();
    for row in rows {
        let (stored, rowid) = row.map_err(error)?;
        next_rowid(AppendOnlyTable::Audits, &position, rowid)?;
        let (event, record_json, sha256) = decode_audit_row(stored)?;
        let evidence = verify_loaded_audit(
            connection,
            topology,
            replica_id,
            event,
            &record_json,
            sha256,
        )?;
        attempts.insert(evidence.event.attempt_id);
        position = TablePosition {
            rowid,
            sha256: Some(evidence.sha256),
        };
    }
    for attempt_id in attempts {
        load_attempt_audits(connection, topology, replica_id, &attempt_id)?;
    }
    Ok(position)
}

fn verify_audit_sequences(audits: &[ExchangeAuditEvidence]) -> DurableResult<()> {
    audit_sequence_problem(audits.iter().map(|entry| &entry.event))
        .map_or(Ok(()), |detail| Err(DurableError::Corrupt(detail)))
}

fn verify_audits(
    connection: &Connection,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<()> {
    load_audits(connection, topology, replica_id).map(|_| ())
}

fn load_receipt_evidence(connection: &Connection) -> DurableResult<Vec<ReceiptEvidence>> {
    let mut statement = connection
        .prepare("SELECT operation_id, kind, source_replica_id, wire_operation_id, sha256 FROM receipts ORDER BY operation_id")
        .map_err(error)?;
    let evidence = statement
        .query_map([], |row| {
            Ok(ReceiptEvidence {
                operation_id: row.get(0)?,
                kind: enum_from_text(&row.get::<_, String>(1)?).map_err(|problem| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(problem),
                    )
                })?,
                source_replica_id: row.get(2)?,
                wire_operation_id: row.get(3)?,
                sha256: row.get(4)?,
            })
        })
        .map_err(error)?
        .map(|row| row.map_err(error))
        .collect();
    evidence
}

fn incomplete_attempts(audits: &[ExchangeAuditEvidence]) -> DurableResult<Vec<IncompleteAttempt>> {
    verify_audit_sequences(audits)?;
    let mut attempts: BTreeMap<(AuditDirection, String), &ExchangeAuditEvent> = BTreeMap::new();
    for evidence in audits {
        let event = &evidence.event;
        let key = (event.direction, event.attempt_id.clone());
        if attempts.get(&key).map_or(true, |prior| {
            phase_rank(event.phase) > phase_rank(prior.phase)
        }) {
            attempts.insert(key, event);
        }
    }
    Ok(attempts
        .into_values()
        .filter(|event| {
            !matches!(
                event.phase,
                AuditPhase::OutboundExchangeCompleted
                    | AuditPhase::InboundReplyWriteObserved
                    | AuditPhase::InboundDiagnosticReplyWritten
                    | AuditPhase::InboundConnectionClosed
            )
        })
        .map(|event| IncompleteAttempt {
            direction: event.direction,
            attempt_id: event.attempt_id.clone(),
            wire_nonce: event.wire_nonce.clone(),
            wire_operation_id: event.operation_id.clone(),
            last_phase: event.phase,
        })
        .collect())
}

fn phase_rank(phase: AuditPhase) -> u8 {
    match phase {
        AuditPhase::OutboundRequestPrepared | AuditPhase::InboundRequestObserved => 0,
        AuditPhase::InboundImportCommitted | AuditPhase::InboundRefusalRecorded => 1,
        AuditPhase::InboundReplyPrepared => 2,
        AuditPhase::OutboundExchangeCompleted
        | AuditPhase::InboundReplyWriteObserved
        | AuditPhase::InboundDiagnosticReplyWritten
        | AuditPhase::InboundConnectionClosed => 3,
    }
}

/// Verifies an existing manager store through a read-only SQLite connection.
/// This operation never creates, migrates, checkpoints or repairs a database.
///
/// # Errors
/// Rejects missing/non-regular stores, schema or identity mismatch, SQLite
/// integrity failure, a table whose rowids are not contiguous from 1, and any
/// corrupt fact, receipt, audit or receipt link.
pub fn inspect_read_only(
    path: &Path,
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<CanonicalStoreInspection> {
    require_regular_nonsymlink(path)?;
    let topology = validate_local_configuration(configuration, replica_id)?;
    let (_source, mut connection) = open_inspection_source(path)?;
    let (transaction, version) = begin_inspection(&mut connection, &topology, replica_id)?;
    let integrity: String = transaction
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(error)?;
    if integrity != "ok" {
        return Err(DurableError::Corrupt(
            "SQLite integrity check failed".into(),
        ));
    }
    for table in AppendOnlyTable::ALL {
        verify_contiguous_rowids(&transaction, table)?;
    }
    verify_receipts(&transaction, topology.logical_manager_id())?;
    let replica = load(&transaction, &topology, replica_id)?;
    let audits = load_audits(&transaction, &topology, replica_id)?;
    let receipts = load_receipt_evidence(&transaction)?;
    let ordered_facts: Vec<_> = replica.history.values().cloned().collect();
    let logical_history_sha256 = logical_history_sha256(&topology, &ordered_facts)?;
    let receipt_set_sha256 = digest(&json(&("podmesh-manager-ha-receipt-set/1", &receipts))?);
    let audit_set_sha256 = digest(&json(&("podmesh-manager-ha-audit-set/1", &audits))?);
    let incomplete_attempts = incomplete_attempts(&audits)?;
    let audited_import_receipts: std::collections::BTreeSet<_> = audits
        .iter()
        .filter(|entry| entry.event.phase == AuditPhase::InboundImportCommitted)
        .filter_map(|entry| entry.event.local_receipt_operation_id.clone())
        .collect();
    let unaudited_import_receipt_ids = receipts
        .iter()
        .filter(|receipt| {
            matches!(
                receipt.kind,
                ReceiptKind::LaboratoryImport | ReceiptKind::AuthenticatedImport
            ) && !audited_import_receipts.contains(&receipt.operation_id)
        })
        .map(|receipt| receipt.operation_id.clone())
        .collect();
    let view = replica.materialize();
    let inspection = CanonicalStoreInspection {
        schema_version: version,
        logical_manager_id: topology.logical_manager_id().into(),
        replica_id: replica_id.into(),
        history_count: ordered_facts.len(),
        ordered_facts,
        logical_history_sha256,
        current: view.current.into_values().collect(),
        conflicts: view.conflicts,
        blocked_exclusive_resources: view.blocked_exclusive_resources.into_iter().collect(),
        receipt_count: receipts.len(),
        ordered_receipts: receipts,
        receipt_set_sha256,
        audit_event_count: audits.len(),
        ordered_audit_events: audits,
        audit_set_sha256,
        incomplete_attempts,
        unaudited_import_receipt_ids,
        sqlite_integrity_result: integrity,
    };
    transaction.commit().map_err(error)?;
    Ok(inspection)
}

/// Verifies and returns the facts of an existing manager store through the same
/// stable private copy as [`inspect_read_only`]: schema version and shape, the
/// replica and topology identity, the rowid rule of the facts table, and every
/// fact's checksum, JSON, event identity and validation by the reducer. It reads
/// no receipt and no audit row and runs no SQLite `integrity_check`, so neither
/// its verification nor its result grows with the exchange audit table; capturing
/// the private copy still reads the whole database file twice. It is a read of
/// the facts, not a verdict on the store: corrupt receipts or audit rows leave its
/// result unchanged.
///
/// # Errors
/// Rejects missing/non-regular stores, schema or identity mismatch, a facts
/// table whose rowids are not contiguous from 1, and any corrupt fact.
pub fn inspect_facts_read_only(
    path: &Path,
    configuration: &Configuration,
    replica_id: &str,
) -> DurableResult<FactsInspection> {
    require_regular_nonsymlink(path)?;
    let topology = validate_local_configuration(configuration, replica_id)?;
    let (_source, mut connection) = open_inspection_source(path)?;
    let (transaction, _) = begin_inspection(&mut connection, &topology, replica_id)?;
    verify_contiguous_rowids(&transaction, AppendOnlyTable::Facts)?;
    let replica = load(&transaction, &topology, replica_id)?;
    transaction.commit().map_err(error)?;
    let ordered_facts: Vec<_> = replica.history.into_values().collect();
    Ok(FactsInspection {
        history_count: ordered_facts.len(),
        logical_history_sha256: logical_history_sha256(&topology, &ordered_facts)?,
        ordered_facts,
    })
}

/// What an inspection holds while it reads: the private copy it reads, made while
/// this process held no connection on the store, or the count of its read-only
/// connection on a store this process has open, whose files are not copied because
/// copying them would release that process's locks.
struct InspectionSource {
    _copy: Option<ReadOnlySnapshot>,
    _live: Option<LiveConnection>,
}

/// Opens the connection an inspection reads through: a stable private copy of the
/// store, or, when this process has the store open, a read-only connection to the
/// store itself, whose read transaction is then the inspection's snapshot.
fn open_inspection_source(path: &Path) -> DurableResult<(InspectionSource, Connection)> {
    let live = live_file(path)?;
    let open_file = lock(&live.gate, "live store lock")?;
    if live.connections.load(Ordering::SeqCst) == 0 {
        let (snapshot, connection) = open_read_only_snapshot(path)?;
        return Ok((
            InspectionSource {
                _copy: Some(snapshot),
                _live: None,
            },
            connection,
        ));
    }
    let before = fs::symlink_metadata(path).map_err(error)?;
    if *open_file != Some(file_identity(&before)) {
        return Err(changed_live_file());
    }
    let counted = LiveConnection::count(&live);
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    if file_identity(&fs::symlink_metadata(path).map_err(error)?) != file_identity(&before) {
        return Err(changed_live_file());
    }
    Ok((
        InspectionSource {
            _copy: None,
            _live: Some(counted),
        },
        connection,
    ))
}

/// Opens the read transaction of a read-only inspection of a private copy and
/// checks what every inspection rests on: schema version 3, the exact schema
/// shape, and the configured replica and topology identity. Returns the schema
/// version with the transaction.
fn begin_inspection<'c>(
    connection: &'c mut Connection,
    topology: &Topology,
    replica_id: &str,
) -> DurableResult<(Transaction<'c>, u32)> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(error)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(error)?;
    let version: u32 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(error)?;
    if version != 3 {
        return Err(DurableError::Refused(RefusalReason::UnsupportedSchema));
    }
    verify_immutable_schema(&transaction)?;
    let identity: (String, String) = transaction
        .query_row(
            "SELECT replica_id, topology_json FROM identity WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(error)?;
    if identity != (replica_id.to_string(), json(topology)?) {
        return Err(DurableError::Refused(RefusalReason::IdentityMismatch));
    }
    Ok((transaction, version))
}

/// The domain-separated digest of a replica's ordered facts, comparable across
/// converged replicas of one logical manager and topology.
fn logical_history_sha256(topology: &Topology, ordered_facts: &[Fact]) -> DurableResult<String> {
    let topology_sha256 = digest(&json(topology)?);
    Ok(digest(&json(&(
        "podmesh-manager-ha-logical-history/1",
        topology.logical_manager_id(),
        &topology_sha256,
        ordered_facts,
    ))?))
}

struct ReadOnlySnapshot {
    directory: PathBuf,
    database: PathBuf,
}

/// The database file and its sidecars, in the order a capture reads them.
const STORE_FILE_SUFFIXES: [&str; 3] = ["", "-wal", "-shm"];

/// A store whose files total at most this many bytes is captured in memory.
const CAPTURE_IN_MEMORY_BYTES: u64 = 16 << 20;

/// The buffers through which a larger capture streams its files.
const CAPTURE_CHUNK_BYTES: usize = 1 << 20;

impl ReadOnlySnapshot {
    /// Captures a stable private copy of the database, WAL and SHM: the files are
    /// read twice and kept only when both reads are equal, so no writer changed
    /// them between the two reads. A store whose files total at most
    /// `CAPTURE_IN_MEMORY_BYTES` is read twice into memory, which keeps the two
    /// reads of a small store closest together. A larger one is copied into the
    /// private directory, then read again and compared with that copy through
    /// fixed buffers, so that no capture holds memory proportional to the store;
    /// the two reads of a large store are closer together that way too. A capture
    /// whose reads differ starts again, three times at most.
    fn capture(path: &Path) -> DurableResult<Self> {
        let size = store_files_size(path)?;
        for _ in 0..3 {
            let captured = if size <= CAPTURE_IN_MEMORY_BYTES {
                capture_in_memory(path)?
            } else {
                capture_by_copy(path)?
            };
            if let Some(snapshot) = captured {
                return Ok(snapshot);
            }
        }
        Err(DurableError::Storage(
            "manager store changed while capturing a read-only inspection snapshot".into(),
        ))
    }

    fn in_new_directory(path: &Path) -> DurableResult<Self> {
        let directory = secure_snapshot_directory(path)?;
        Ok(Self {
            database: directory.join("store.sqlite"),
            directory,
        })
    }

    /// Creates the private file of one store file, named by its suffix.
    fn create_file(&self, suffix: &str) -> DurableResult<(PathBuf, fs::File)> {
        let copy = self.directory.join(format!("store.sqlite{suffix}"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&copy)
            .map_err(error)?;
        Ok((copy, file))
    }
}

/// Reads the store files twice into memory, and writes them into a new private
/// directory when both reads are equal.
fn capture_in_memory(path: &Path) -> DurableResult<Option<ReadOnlySnapshot>> {
    let files = read_store_files(path)?;
    if read_store_files(path)? != files {
        return Ok(None);
    }
    let snapshot = ReadOnlySnapshot::in_new_directory(path)?;
    for (suffix, bytes) in files {
        snapshot
            .create_file(suffix)?
            .1
            .write_all(&bytes)
            .map_err(error)?;
    }
    Ok(Some(snapshot))
}

/// Copies the store files into a new private directory, whose files are created
/// before the first read, then reads the source again and compares it with the
/// copy.
fn capture_by_copy(path: &Path) -> DurableResult<Option<ReadOnlySnapshot>> {
    let snapshot = ReadOnlySnapshot::in_new_directory(path)?;
    let mut copies = STORE_FILE_SUFFIXES
        .iter()
        .map(|suffix| {
            snapshot
                .create_file(suffix)
                .map(|(copy, file)| (*suffix, copy, file))
        })
        .collect::<DurableResult<Vec<_>>>()?;
    let mut chunks = (
        vec![0_u8; CAPTURE_CHUNK_BYTES],
        vec![0_u8; CAPTURE_CHUNK_BYTES],
    );
    let mut present = Vec::with_capacity(copies.len());
    for (suffix, _, file) in &mut copies {
        present.push(copy_store_file(path, suffix, file, &mut chunks.0)?);
    }
    for ((suffix, _, file), present) in copies.iter_mut().zip(&present) {
        if !same_store_file(path, suffix, file, *present, &mut chunks)? {
            return Ok(None);
        }
    }
    // A sidecar absent from both reads has no copy either.
    for ((_, copy, _), present) in copies.iter().zip(&present) {
        if !present {
            fs::remove_file(copy).map_err(error)?;
        }
    }
    Ok(Some(snapshot))
}

fn secure_snapshot_directory(path: &Path) -> DurableResult<PathBuf> {
    let sequence = NEXT_INSPECTION_SNAPSHOT.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)?
        .as_nanos();
    let entropy = digest(&format!(
        "{}:{}:{sequence}:{timestamp}:{}",
        std::process::id(),
        path.as_os_str().to_string_lossy(),
        random_hex_16()?,
    ));
    let directory =
        std::env::temp_dir().join(format!("podmesh-manager-inspect-{}", &entropy[..24]));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(error)?;
    Ok(directory)
}

fn open_read_only_snapshot(path: &Path) -> DurableResult<(ReadOnlySnapshot, Connection)> {
    match open_snapshot_connection(ReadOnlySnapshot::capture(path)?) {
        Ok(snapshot) => Ok(snapshot),
        Err(DurableError::Storage(_)) => {
            materialize_private_snapshot(ReadOnlySnapshot::capture(path)?)
        }
        Err(problem) => Err(problem),
    }
}

fn materialize_private_snapshot(
    mut snapshot: ReadOnlySnapshot,
) -> DurableResult<(ReadOnlySnapshot, Connection)> {
    let materialized = snapshot.directory.join("materialized.sqlite");
    let source = Connection::open_with_flags(
        &snapshot.database,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    source.busy_timeout(Duration::from_secs(5)).map_err(error)?;
    source
        .execute("VACUUM INTO ?1", [materialized.to_string_lossy().as_ref()])
        .map_err(error)?;
    drop(source);
    snapshot.database = materialized;
    open_snapshot_connection(snapshot)
}

fn open_snapshot_connection(
    snapshot: ReadOnlySnapshot,
) -> DurableResult<(ReadOnlySnapshot, Connection)> {
    let connection = Connection::open_with_flags(
        &snapshot.database,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(error)?;
    connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(error)?;
    connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, u32>(0)
        })
        .map_err(error)?;
    Ok((snapshot, connection))
}

impl Drop for ReadOnlySnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Checks the database and sidecar paths before a capture creates anything, and
/// returns the size of the files present. A missing database, or a database or
/// sidecar path that is not a regular file, is refused; each file is checked again
/// when the capture opens it.
fn store_files_size(path: &Path) -> DurableResult<u64> {
    let mut size = 0_u64;
    for suffix in STORE_FILE_SUFFIXES {
        match fs::symlink_metadata(sidecar_path(path, suffix)) {
            Ok(metadata) if metadata.file_type().is_file() => {
                size = size.saturating_add(metadata.len());
            }
            Ok(_) => return Err(DurableError::Refused(RefusalReason::UnsafeStore)),
            Err(problem)
                if problem.kind() == std::io::ErrorKind::NotFound && !suffix.is_empty() => {}
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                return Err(DurableError::Refused(RefusalReason::MissingStore));
            }
            Err(problem) => return Err(error(problem)),
        }
    }
    Ok(size)
}

/// Reads the database and each existing sidecar into memory.
fn read_store_files(path: &Path) -> DurableResult<Vec<(&'static str, Vec<u8>)>> {
    let mut files = Vec::with_capacity(STORE_FILE_SUFFIXES.len());
    for suffix in STORE_FILE_SUFFIXES {
        let source_path = sidecar_path(path, suffix);
        let Some((mut source, opened)) =
            open_regular_file_nofollow(&source_path, !suffix.is_empty())?
        else {
            continue;
        };
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).map_err(error)?;
        confirm_same_file(&source_path, &opened)?;
        files.push((suffix, bytes));
    }
    Ok(files)
}

/// Copies one store file, the database or a sidecar named by its suffix, into
/// its private copy. Returns `false` for an absent sidecar, whose copy stays
/// empty.
fn copy_store_file(
    path: &Path,
    suffix: &str,
    copy: &mut fs::File,
    chunk: &mut [u8],
) -> DurableResult<bool> {
    let source_path = sidecar_path(path, suffix);
    let Some((mut source, opened)) = open_regular_file_nofollow(&source_path, !suffix.is_empty())?
    else {
        return Ok(false);
    };
    loop {
        let read = fill_chunk(&mut source, chunk)?;
        if read == 0 {
            break;
        }
        copy.write_all(&chunk[..read]).map_err(error)?;
    }
    confirm_same_file(&source_path, &opened)?;
    Ok(true)
}

/// Reads one store file again and compares it with its private copy: the same
/// presence, the same length and the same bytes.
fn same_store_file(
    path: &Path,
    suffix: &str,
    copy: &mut fs::File,
    copied: bool,
    (read_again, copied_bytes): &mut (Vec<u8>, Vec<u8>),
) -> DurableResult<bool> {
    let source_path = sidecar_path(path, suffix);
    let Some((mut source, opened)) = open_regular_file_nofollow(&source_path, !suffix.is_empty())?
    else {
        return Ok(!copied);
    };
    if !copied {
        return Ok(false);
    }
    copy.seek(SeekFrom::Start(0)).map_err(error)?;
    let same = loop {
        let read = fill_chunk(&mut source, read_again)?;
        if fill_chunk(copy, copied_bytes)? != read || read_again[..read] != copied_bytes[..read] {
            break false;
        }
        if read == 0 {
            break true;
        }
    };
    confirm_same_file(&source_path, &opened)?;
    Ok(same)
}

/// Reads into `chunk` until it is full or the file ends; returns the bytes read.
fn fill_chunk(file: &mut fs::File, chunk: &mut [u8]) -> DurableResult<usize> {
    let mut filled = 0;
    while filled < chunk.len() {
        match file.read(&mut chunk[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(problem) if problem.kind() == std::io::ErrorKind::Interrupted => {}
            Err(problem) => return Err(error(problem)),
        }
    }
    Ok(filled)
}

/// Opens a regular file without following a symlink, with the metadata of the
/// opened descriptor, refusing a path that is not a regular file or whose
/// identity changed between its `lstat` and its open. A missing file is absent
/// when `missing_is_absent`, and a missing store otherwise.
fn open_regular_file_nofollow(
    path: &Path,
    missing_is_absent: bool,
) -> DurableResult<Option<(fs::File, fs::Metadata)>> {
    // PodMesh targets Linux hosts. These are the Linux ABI values for
    // O_NOFOLLOW and O_NONBLOCK; no unsafe code or additional dependency is used.
    const O_NOFOLLOW_NONBLOCK: i32 = 0o404_000;
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => return Err(DurableError::Refused(RefusalReason::UnsafeStore)),
        Err(problem) if missing_is_absent && problem.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
            return Err(DurableError::Refused(RefusalReason::MissingStore));
        }
        Err(problem) => return Err(error(problem)),
    };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW_NONBLOCK)
        .open(path)
        .map_err(|problem| match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                DurableError::Refused(RefusalReason::UnsafeStore)
            }
            _ => error(problem),
        })?;
    let opened = file.metadata().map_err(error)?;
    if !opened.file_type().is_file() || opened.dev() != before.dev() || opened.ino() != before.ino()
    {
        return Err(DurableError::Refused(RefusalReason::UnsafeStore));
    }
    Ok(Some((file, opened)))
}

/// Refuses a path that no longer names the regular file that was opened.
fn confirm_same_file(path: &Path, opened: &fs::Metadata) -> DurableResult<()> {
    let after = fs::symlink_metadata(path).map_err(error)?;
    if !after.file_type().is_file() || after.dev() != opened.dev() || after.ino() != opened.ino() {
        return Err(DurableError::Refused(RefusalReason::UnsafeStore));
    }
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn random_hex_16() -> DurableResult<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(error)?;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

fn require_regular_nonsymlink(path: &Path) -> DurableResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(DurableError::Refused(RefusalReason::UnsafeStore)),
        Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
            Err(DurableError::Refused(RefusalReason::MissingStore))
        }
        Err(problem) => Err(error(problem)),
    }
}

const SCHEMA: &str = "
CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), replica_id TEXT NOT NULL, topology_json TEXT NOT NULL);
CREATE TABLE facts (event_id TEXT PRIMARY KEY, fact_json TEXT NOT NULL, sha256 TEXT NOT NULL);
CREATE TABLE receipts (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    source_replica_id TEXT,
    wire_operation_id TEXT,
    request_json TEXT NOT NULL,
    response_json TEXT NOT NULL,
    sha256 TEXT NOT NULL
);
CREATE TABLE exchange_audit_events (
    audit_event_id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL,
    wire_nonce TEXT NOT NULL,
    direction TEXT NOT NULL,
    phase TEXT NOT NULL,
    authenticated_peer_id TEXT,
    peer_claim TEXT,
    operation_id TEXT,
    request_frame_bytes INTEGER NOT NULL,
    request_announced_body_bytes INTEGER,
    request_sha256 TEXT,
    reply_frame_bytes INTEGER NOT NULL,
    reply_announced_body_bytes INTEGER,
    reply_sha256 TEXT,
    outcome TEXT NOT NULL,
    error_category TEXT,
    reason_code TEXT,
    local_receipt_operation_id TEXT,
    local_receipt_sha256 TEXT,
    remote_receipt_operation_id TEXT,
    remote_receipt_sha256 TEXT,
    replayed INTEGER NOT NULL CHECK(replayed IN (0, 1)),
    record_json TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    UNIQUE(direction, attempt_id, phase)
);
CREATE TRIGGER facts_no_update BEFORE UPDATE ON facts BEGIN SELECT RAISE(ABORT, 'immutable fact'); END;
CREATE TRIGGER facts_no_delete BEFORE DELETE ON facts BEGIN SELECT RAISE(ABORT, 'immutable fact'); END;
CREATE TRIGGER identity_no_update BEFORE UPDATE ON identity BEGIN SELECT RAISE(ABORT, 'immutable identity'); END;
CREATE TRIGGER identity_no_delete BEFORE DELETE ON identity BEGIN SELECT RAISE(ABORT, 'immutable identity'); END;
CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;
CREATE TRIGGER receipts_no_delete BEFORE DELETE ON receipts BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;
CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;
CREATE TRIGGER exchange_audit_events_no_delete BEFORE DELETE ON exchange_audit_events BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;
PRAGMA user_version=3;
";

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(length: usize, seed: usize) -> Vec<u8> {
        (0..length)
            .map(|index| u8::try_from((index + seed) % 251).unwrap())
            .collect()
    }

    /// The stores of the other tests are small, so their captures run in memory:
    /// the capture by copy of a larger store is exercised here, over several chunks.
    #[test]
    fn a_capture_by_copy_keeps_the_bytes_of_the_files_present() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let database = bytes(3 * CAPTURE_CHUNK_BYTES + 17, 0);
        let wal = bytes(CAPTURE_CHUNK_BYTES, 7);
        fs::write(&path, &database).unwrap();
        fs::write(sidecar_path(&path, "-wal"), &wal).unwrap();
        let snapshot = capture_by_copy(&path).unwrap().unwrap();
        assert_eq!(fs::read(&snapshot.database).unwrap(), database);
        assert_eq!(
            fs::read(snapshot.directory.join("store.sqlite-wal")).unwrap(),
            wal
        );
        assert!(!snapshot.directory.join("store.sqlite-shm").exists());
        let private = snapshot.directory.clone();
        drop(snapshot);
        assert!(!private.exists());
    }

    /// The second read of a capture by copy keeps the copy only when the source
    /// file still holds the same bytes, and a sidecar is present in both reads or
    /// in neither.
    #[test]
    fn a_capture_by_copy_refuses_a_file_that_changed_between_its_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let original = bytes(2 * CAPTURE_CHUNK_BYTES + 5, 0);
        let copy_path = directory.path().join("copy");
        let mut chunks = (
            vec![0_u8; CAPTURE_CHUNK_BYTES],
            vec![0_u8; CAPTURE_CHUNK_BYTES],
        );
        let mut compare = |source: &[u8], copied: bool| {
            fs::write(&copy_path, &original).unwrap();
            let mut copy = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&copy_path)
                .unwrap();
            fs::write(&path, source).unwrap();
            same_store_file(&path, "", &mut copy, copied, &mut chunks).unwrap()
        };
        assert!(compare(&original, true));
        let mut changed = original.clone();
        changed[CAPTURE_CHUNK_BYTES + 3] ^= 1;
        assert!(!compare(&changed, true));
        assert!(!compare(&original[..original.len() - 1], true));
        let mut longer = original.clone();
        longer.push(0);
        assert!(!compare(&longer, true));
        assert!(!compare(&original, false));

        let mut copy = fs::File::open(&copy_path).unwrap();
        assert!(same_store_file(&path, "-wal", &mut copy, false, &mut chunks).unwrap());
        assert!(!same_store_file(&path, "-wal", &mut copy, true, &mut chunks).unwrap());
    }
}
