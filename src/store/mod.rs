//! The durable store under a PodMesh journal: one contract, two engines.
//!
//! Every operation of this node writes what it did into a journal, and the journal is what the
//! next operation reads to know what it may do. Until now that journal was SQLite and nothing
//! else: `rusqlite::Connection` is passed by hand from `lib.rs` into every module, and each
//! module writes SQLite's dialect. The storage plan (`docs/STORAGE-MARIADB-MIGRATION-PLAN.md`)
//! moves the node and the manager onto their own MariaDB instances, one per functional role,
//! and that move needs a place where the engine is named once instead of everywhere.
//!
//! This module is that place. [`DurableStore`] is what a caller needs from a journal -- a
//! statement that changes rows, a statement that returns them, a transaction that either
//! commits whole or leaves nothing, an integrity check it can report, and named faults instead
//! of an engine's error codes. [`sqlite::SqliteStore`] is the engine every node runs today;
//! [`mariadb::MariadbStore`] is the engine the plan moves to, built only when this crate is
//! built with the `mariadb` feature.
//!
//! Phase 2 brought the node's **schema** here and left its **call sites** where they were.
//! [`migrations`] carries every production table of the node journal as ordered SQL, one file per
//! engine, and `open_state` applies them to whichever store the profile names: the journal exists
//! in the same shape on both engines, at a version the store records. What still speaks SQLite is
//! everything above the journal -- `handle` and the modules under it take a `rusqlite::Connection`
//! -- so a node whose profile says `mariadb` opens, migrates, and is then refused by name rather
//! than served from a file. [`catalog`] is where the last of the dialect went: a caller asks what
//! the store carries instead of reading `sqlite_master`.
//!
//! Two conventions hold across engines, because the two dialects do not agree:
//!
//! * **Placeholders are `?`, bound by position.** `?1` is SQLite's alone and never appears here.
//! * **Values are the five SQLite storage classes** ([`Value`]). A MariaDB column is read back
//!   into the same five, so a caller written against one engine reads the same shapes on the
//!   other. What a column may hold is the schema's business, not this layer's.
//!
//! DDL is not portable and this module does not pretend it is: [`bootstrap`] writes its own
//! schema table in each engine's dialect, and [`migrations`] carries the node's tables as one
//! ordered set with a file per engine.
pub mod catalog;
pub mod config;
#[cfg(feature = "mariadb")]
pub mod mariadb;
pub mod migrations;
pub mod sqlite;

pub use config::{Engine, MariadbConfig, SqliteConfig, StoreConfig};
#[cfg(feature = "mariadb")]
pub use mariadb::MariadbStore;
pub use sqlite::SqliteStore;

use serde_json::{json, Value as Json};
use std::{fmt, sync::Arc};

pub type Result<T> = std::result::Result<T, StoreError>;

/// What went wrong, named so that a caller decides without reading an engine's error codes.
///
/// The distinction that matters to a caller is [`Fault::Busy`] and [`Fault::Locked`]: the store
/// refused this attempt and the same attempt may succeed later. SQLite says `SQLITE_BUSY` when
/// another writer holds the file and `SQLITE_LOCKED` inside its own connection; MariaDB says
/// `1205` when a row lock waited past its timeout and `1213` when InnoDB broke a deadlock. They
/// are the same fact for the caller, and [`Fault::retryable`] is the question it asks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// Another writer holds what this statement needs; the wait ran out.
    Busy,
    /// The store refused this attempt to break a lock cycle of its own.
    Locked,
    /// The store could not be reached, opened, or is not a store at all.
    Unavailable,
    /// The credentials were refused, or they do not carry this right.
    Denied,
    /// The statement names a table or a column the store does not have.
    Schema,
    /// The store contradicts itself, or the statement would make it contradict itself.
    Integrity,
    /// The value is not of the kind the caller asked for.
    Type,
    /// This build, or this engine, does not carry what was asked.
    Unsupported,
    /// Anything else the engine reported; the message carries what it said.
    Other,
}

impl Fault {
    /// Whether the same attempt is worth making again: the store refused it, it did not fail it.
    pub fn retryable(self) -> bool {
        matches!(self, Fault::Busy | Fault::Locked)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Fault::Busy => "busy",
            Fault::Locked => "locked",
            Fault::Unavailable => "unavailable",
            Fault::Denied => "denied",
            Fault::Schema => "schema",
            Fault::Integrity => "integrity",
            Fault::Type => "type",
            Fault::Unsupported => "unsupported",
            Fault::Other => "other",
        }
    }

    pub fn error(self, message: impl Into<String>) -> StoreError {
        StoreError::new(self, message)
    }
}

#[derive(Clone)]
pub struct StoreError {
    pub fault: Fault,
    pub message: String,
}

/// The same sentence as [`fmt::Display`]. A refusal reaches an operator through whichever of the
/// two the caller happened to use -- `main` returning `Box<dyn Error>` prints the debug form --
/// and a node that refuses to start says why in words either way.
impl fmt::Debug for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl StoreError {
    pub fn new(fault: Fault, message: impl Into<String>) -> Self {
        Self { fault, message: message.into() }
    }

    pub fn retryable(&self) -> bool {
        self.fault.retryable()
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "store {}: {}", self.fault.as_str(), self.message)
    }
}

impl std::error::Error for StoreError {}

/// A value as it crosses the store boundary: SQLite's five storage classes, which MariaDB's
/// columns are read back into.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn integer(&self) -> Option<i64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    pub fn real(&self) -> Option<f64> {
        match self {
            Value::Real(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn blob(&self) -> Option<&[u8]> {
        match self {
            Value::Blob(b) => Some(b),
            Value::Text(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Integer(_) => "integer",
            Value::Real(_) => "real",
            Value::Text(_) => "text",
            Value::Blob(_) => "blob",
        }
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Integer(value)
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::Integer(value as i64)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Integer(value as i64)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Integer(i64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Real(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Value::Text(value.clone())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Value::Blob(value)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Value::Null, Into::into)
    }
}

/// One row of a result, with the column names the statement returned them under.
#[derive(Clone, Debug)]
pub struct Row {
    columns: Arc<Vec<String>>,
    values: Vec<Value>,
}

impl Row {
    pub fn new(columns: Arc<Vec<String>>, values: Vec<Value>) -> Self {
        Self { columns, values }
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn into_values(self) -> Vec<Value> {
        self.values
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn value(&self, index: usize) -> Result<&Value> {
        self.values
            .get(index)
            .ok_or_else(|| Fault::Type.error(format!("the row has {} columns, not {}", self.values.len(), index + 1)))
    }

    /// The value under a column name, for a statement whose column order is not the contract.
    pub fn named(&self, name: &str) -> Result<&Value> {
        let index = self
            .columns
            .iter()
            .position(|column| column == name)
            .ok_or_else(|| Fault::Type.error(format!("the row carries no column named {name}")))?;
        self.value(index)
    }

    pub fn integer(&self, index: usize) -> Result<i64> {
        let value = self.value(index)?;
        value.integer().ok_or_else(|| Fault::Type.error(format!("column {index} is {}, not an integer", value.kind())))
    }

    pub fn text(&self, index: usize) -> Result<&str> {
        let value = self.value(index)?;
        value.text().ok_or_else(|| Fault::Type.error(format!("column {index} is {}, not text", value.kind())))
    }

    pub fn blob(&self, index: usize) -> Result<&[u8]> {
        let value = self.value(index)?;
        value.blob().ok_or_else(|| Fault::Type.error(format!("column {index} is {}, not bytes", value.kind())))
    }
}

/// What an integrity check found, in the shape the qualification scripts read
/// (`store_integrity_result`, which the plan puts beside the legacy `sqlite_integrity_result`).
#[derive(Clone, Debug)]
pub struct Integrity {
    pub engine: Engine,
    pub ok: bool,
    /// What was checked: the whole database for SQLite, every base table for MariaDB.
    pub checked: Vec<String>,
    /// What the engine said when it was not `ok`; empty otherwise.
    pub findings: Vec<String>,
}

impl Integrity {
    pub fn result(&self) -> &'static str {
        if self.ok {
            "ok"
        } else {
            "faulty"
        }
    }

    pub fn to_json(&self) -> Json {
        json!({
            "engine": self.engine.as_str(),
            "store_integrity_result": self.result(),
            "checked": self.checked,
            "findings": self.findings,
        })
    }
}

/// A journal this node can write to and read back, whatever engine carries it.
///
/// Every statement takes `?` placeholders bound by position. A method takes `&mut self` because
/// a connection is a conversation: one statement at a time, in order, on one connection.
pub trait DurableStore: Send {
    fn engine(&self) -> Engine;

    /// A statement that changes rows; the count the engine reports.
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64>;

    /// Statements separated by `;`, with no parameters: schema work, pragmas, session settings.
    fn execute_batch(&mut self, sql: &str) -> Result<()>;

    /// A statement that returns rows, collected before this call returns.
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>>;

    /// The one row a statement was expected to return, or none.
    fn query_one(&mut self, sql: &str, params: &[Value]) -> Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// A transaction that commits whole or leaves nothing. Dropped without a decision, it rolls
    /// back: a caller that returns early through `?` never half-writes.
    fn transaction(&mut self) -> Result<Box<dyn Transaction + '_>>;

    /// What the engine says about its own consistency.
    fn integrity_check(&mut self) -> Result<Integrity>;

    /// The base tables this store carries, so that a caller never reads `sqlite_master` itself.
    fn tables(&mut self) -> Result<Vec<String>>;
}

/// An open transaction. It is used through [`Transaction::execute`] and
/// [`Transaction::query`], and ended by [`Transaction::commit`] or [`Transaction::rollback`];
/// dropping it rolls back.
pub trait Transaction {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64>;

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>>;

    fn query_one(&mut self, sql: &str, params: &[Value]) -> Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    fn commit(self: Box<Self>) -> Result<()>;

    fn rollback(self: Box<Self>) -> Result<()>;
}

/// Open the store a configuration names.
///
/// A configuration that names MariaDB in a build without the `mariadb` feature is refused by
/// name rather than silently opened as SQLite: a node that believes it writes to MariaDB and
/// writes to a file instead is the one failure this layer must never allow.
pub fn open(config: &StoreConfig) -> Result<Box<dyn DurableStore>> {
    match config.engine {
        Engine::Sqlite => Ok(Box::new(SqliteStore::open(&config.sqlite)?)),
        #[cfg(feature = "mariadb")]
        Engine::Mariadb => Ok(Box::new(MariadbStore::open(&config.mariadb)?)),
        #[cfg(not(feature = "mariadb"))]
        Engine::Mariadb => Err(Fault::Unsupported.error(
            "this build carries no MariaDB backend: build podmesh with the mariadb feature, or set store.engine to sqlite",
        )),
    }
}

/// The table where the store records which schema it carries, and at which version.
pub const SCHEMA_TABLE: &str = "store_schema";

/// The version a store carries after the bootstrap alone, before any node migration is applied.
/// [`migrations::apply`] moves the recorded version on from here, one migration at a time.
pub const BOOTSTRAP_VERSION: i64 = 1;

fn schema_ddl(engine: Engine) -> &'static str {
    match engine {
        // No AUTOINCREMENT and no engine-specific type: the name is the key on both sides.
        Engine::Sqlite => {
            "CREATE TABLE IF NOT EXISTS store_schema(
                name TEXT PRIMARY KEY,
                version INTEGER NOT NULL,
                applied_at INTEGER NOT NULL);"
        }
        // utf8mb4 and InnoDB are named rather than inherited: a store restored on another
        // server must carry the same character set and the same transactional engine.
        Engine::Mariadb => {
            "CREATE TABLE IF NOT EXISTS store_schema(
                name VARCHAR(64) NOT NULL,
                version BIGINT NOT NULL,
                applied_at BIGINT NOT NULL,
                PRIMARY KEY (name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;"
        }
    }
}

/// Make the store carry [`SCHEMA_TABLE`], which is where every schema records its version.
///
/// It is the minimal thing an instance must answer for the node to consider it usable, and it is
/// written to be run again on a store that already has it.
pub fn ensure_schema_table(store: &mut dyn DurableStore) -> Result<()> {
    store.execute_batch(schema_ddl(store.engine()))
}

/// Make the store carry [`SCHEMA_TABLE`], and record that `name` is at `version`.
pub fn bootstrap(store: &mut dyn DurableStore, name: &str, version: i64) -> Result<()> {
    ensure_schema_table(store)?;
    let applied_at = crate::now() as i64;
    // Delete then insert rather than an upsert: `ON CONFLICT` and `ON DUPLICATE KEY` are each
    // one engine's dialect, and the transaction makes the pair atomic on both.
    let mut tx = store.transaction()?;
    tx.execute("DELETE FROM store_schema WHERE name = ?", &[Value::from(name)])?;
    tx.execute(
        "INSERT INTO store_schema(name, version, applied_at) VALUES(?, ?, ?)",
        &[Value::from(name), Value::from(version), Value::from(applied_at)],
    )?;
    tx.commit()
}

/// The MariaDB tests share one server, named by one DSN, and each of them creates and drops
/// tables in it. They take this first so that what one drops is never what another is reading.
#[cfg(all(test, feature = "mariadb"))]
pub(crate) static MARIADB_TEST_SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The version a named schema is recorded at, or none when the store never carried it.
pub fn schema_version(store: &mut dyn DurableStore, name: &str) -> Result<Option<i64>> {
    match store.query_one("SELECT version FROM store_schema WHERE name = ?", &[Value::from(name)])? {
        Some(row) => Ok(Some(row.integer(0)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_decides_which_engine_opens() {
        let dir = std::env::temp_dir().join(format!("podmesh-store-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = StoreConfig::for_state_dir(&dir);
        let mut store = open(&config).unwrap();
        assert_eq!(store.engine(), Engine::Sqlite);
        bootstrap(store.as_mut(), "node", BOOTSTRAP_VERSION).unwrap();
        drop(store);
        assert!(dir.join("state.sqlite").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A build with no MariaDB backend refuses a MariaDB profile by name. It must never fall
    /// back to a file: a node that believes it writes to a server and writes to a file instead
    /// would be right about everything it reads back, and wrong about where its journal is.
    #[cfg(not(feature = "mariadb"))]
    #[test]
    fn a_mariadb_profile_is_refused_by_a_build_that_carries_no_mariadb() {
        let config = StoreConfig { engine: Engine::Mariadb, ..StoreConfig::default() };
        let refused = open(&config).err().expect("a build without the feature opens no MariaDB store");
        assert_eq!(refused.fault, Fault::Unsupported);
        assert!(refused.message.contains("mariadb feature"));
    }
}
