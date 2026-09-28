//! The SQLite backend: the engine every PodMesh node runs today.
//!
//! It wraps `rusqlite` the way `open_state` and every module already use it -- one connection,
//! write-ahead logging, statements prepared on the connection that runs them -- and adds the two
//! things the contract asks for and the bare connection does not give: faults named rather than
//! error codes, and a wait before a busy store is reported busy.
//!
//! `open_state` opens the node's journal through this type and hands the connection back with
//! [`SqliteStore::into_connection`]: the schema arrives as migrations through the contract, and
//! the modules keep the `rusqlite::Connection` they have always been given, on the same file.
use super::config::{Engine, SqliteConfig};
use super::{DurableStore, Fault, Integrity, Result, Row, Transaction, Value};
use rusqlite::{
    types::{ToSqlOutput, ValueRef},
    Connection, ErrorCode,
};
use std::{fs, sync::Arc};

pub struct SqliteStore {
    db: Connection,
}

impl SqliteStore {
    /// Open the file the profile names, making its directory when it is not there yet.
    pub fn open(config: &SqliteConfig) -> Result<Self> {
        if let Some(parent) = config.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    Fault::Unavailable.error(format!("the directory {} could not be made: {e}", parent.display()))
                })?;
            }
        }
        let db = Connection::open(&config.path)
            .map_err(|e| fault_of(&e).error(format!("{} could not be opened: {e}", config.path.display())))?;
        Self::prepare(db, config)
    }

    /// A store no file carries, for a test or a dry run.
    pub fn open_in_memory() -> Result<Self> {
        let db = Connection::open_in_memory().map_err(|e| fault_of(&e).error(format!("a memory store could not be opened: {e}")))?;
        Self::prepare(db, &SqliteConfig { journal_wal: false, ..SqliteConfig::default() })
    }

    /// Answer the contract on a connection another part of the node already opened.
    ///
    /// This is how a caller reads an existing journal through the store without anything being
    /// re-opened underneath it, and how Phase 2 moves a call site at a time: the connection
    /// stays the one `open_state` returned, and the caller gets it back with
    /// [`SqliteStore::into_connection`].
    pub fn adopt(db: Connection) -> Self {
        Self { db }
    }

    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub fn into_connection(self) -> Connection {
        self.db
    }

    fn prepare(db: Connection, config: &SqliteConfig) -> Result<Self> {
        db.busy_timeout(config.busy_timeout)
            .map_err(|e| fault_of(&e).error(format!("the busy timeout was refused: {e}")))?;
        if config.journal_wal {
            // The pragma answers with the mode it settled on, so it is read as a query; a store
            // held in memory answers "memory" and that is not a failure.
            db.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
                .map_err(|e| fault_of(&e).error(format!("write-ahead logging was refused: {e}")))?;
        }
        Ok(Self { db })
    }
}

impl DurableStore for SqliteStore {
    fn engine(&self) -> Engine {
        Engine::Sqlite
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        execute_on(&self.db, sql, params)
    }

    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        self.db.execute_batch(sql).map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        query_on(&self.db, sql, params)
    }

    fn transaction(&mut self) -> Result<Box<dyn Transaction + '_>> {
        let tx = self.db.transaction().map_err(|e| fault_of(&e).error(format!("a transaction could not be opened: {e}")))?;
        Ok(Box::new(SqliteTransaction { tx }))
    }

    fn integrity_check(&mut self) -> Result<Integrity> {
        let rows = query_on(&self.db, "PRAGMA integrity_check", &[])?;
        let findings: Vec<String> = rows
            .iter()
            .filter_map(|row| row.text(0).ok())
            .filter(|finding| *finding != "ok")
            .map(str::to_string)
            .collect();
        Ok(Integrity {
            engine: Engine::Sqlite,
            ok: findings.is_empty(),
            checked: vec![self.db.path().unwrap_or("memory").to_string()],
            findings,
        })
    }

    fn tables(&mut self) -> Result<Vec<String>> {
        let rows = query_on(
            &self.db,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            &[],
        )?;
        rows.iter().map(|row| row.text(0).map(str::to_string)).collect()
    }
}

struct SqliteTransaction<'a> {
    tx: rusqlite::Transaction<'a>,
}

impl Transaction for SqliteTransaction<'_> {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        execute_on(&self.tx, sql, params)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        query_on(&self.tx, sql, params)
    }

    fn commit(self: Box<Self>) -> Result<()> {
        self.tx.commit().map_err(|e| fault_of(&e).error(format!("the transaction could not be committed: {e}")))
    }

    fn rollback(self: Box<Self>) -> Result<()> {
        self.tx.rollback().map_err(|e| fault_of(&e).error(format!("the transaction could not be rolled back: {e}")))
    }
}

fn execute_on(db: &Connection, sql: &str, params: &[Value]) -> Result<u64> {
    db.execute(sql, rusqlite::params_from_iter(params.iter()))
        .map(|changed| changed as u64)
        .map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))
}

fn query_on(db: &Connection, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
    let mut statement = db.prepare(sql).map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))?;
    let columns: Arc<Vec<String>> = Arc::new(statement.column_names().into_iter().map(str::to_string).collect());
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))?;
    let mut collected = Vec::new();
    while let Some(row) = rows.next().map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))? {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let value = row.get_ref(index).map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))?;
            values.push(from_sqlite(value));
        }
        collected.push(Row::new(columns.clone(), values));
    }
    Ok(collected)
}

fn from_sqlite(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Integer(i),
        ValueRef::Real(f) => Value::Real(f),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => Value::Text(text.to_string()),
            Err(_) => Value::Blob(bytes.to_vec()),
        },
        ValueRef::Blob(bytes) => Value::Blob(bytes.to_vec()),
    }
}

impl rusqlite::ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            Value::Null => ToSqlOutput::Borrowed(ValueRef::Null),
            Value::Integer(i) => ToSqlOutput::Borrowed(ValueRef::Integer(*i)),
            Value::Real(f) => ToSqlOutput::Borrowed(ValueRef::Real(*f)),
            Value::Text(text) => ToSqlOutput::Borrowed(ValueRef::Text(text.as_bytes())),
            Value::Blob(bytes) => ToSqlOutput::Borrowed(ValueRef::Blob(bytes)),
        })
    }
}

/// SQLite's error codes as the faults a caller decides on. `SQLITE_BUSY` and `SQLITE_LOCKED`
/// are the two that say "later": everything else says "not like this".
fn fault_of(error: &rusqlite::Error) -> Fault {
    match error {
        rusqlite::Error::SqliteFailure(failure, message) => from_code(failure.code, message.as_deref()),
        rusqlite::Error::SqlInputError { error, msg, .. } => from_code(error.code, Some(msg)),
        _ => Fault::Other,
    }
}

fn from_code(code: ErrorCode, message: Option<&str>) -> Fault {
    match code {
        ErrorCode::DatabaseBusy => Fault::Busy,
        ErrorCode::DatabaseLocked => Fault::Locked,
        ErrorCode::ConstraintViolation => Fault::Integrity,
        ErrorCode::DatabaseCorrupt => Fault::Integrity,
        ErrorCode::CannotOpen | ErrorCode::NotADatabase => Fault::Unavailable,
        ErrorCode::ReadOnly | ErrorCode::PermissionDenied | ErrorCode::AuthorizationForStatementDenied => Fault::Denied,
        ErrorCode::TypeMismatch => Fault::Type,
        _ => match message {
            Some(said) if said.contains("no such table") || said.contains("no such column") => Fault::Schema,
            _ => Fault::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::{bootstrap, schema_version, BOOTSTRAP_VERSION};
    use super::*;
    use std::path::PathBuf;

    /// A directory of this test's own, under the system's scratch: nothing durable lives here.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("podmesh-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn memory() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap()
    }

    #[test]
    fn the_five_value_kinds_come_back_as_they_were_written() {
        let mut store = memory();
        store.execute_batch("CREATE TABLE kinds(rank INTEGER, held BLOB);").unwrap();
        let written = [
            Value::Null,
            Value::Integer(-7),
            Value::Real(0.5),
            Value::Text("a universe".into()),
            Value::Blob(vec![0, 159, 146, 150]),
        ];
        for (index, value) in written.iter().enumerate() {
            store.execute("INSERT INTO kinds(rank, held) VALUES(?, ?)", &[Value::from(index as i64), value.clone()]).unwrap();
        }
        let rows = store.query("SELECT held FROM kinds ORDER BY rank", &[]).unwrap();
        let read: Vec<Value> = rows.iter().map(|row| row.value(0).unwrap().clone()).collect();
        assert_eq!(read, written.to_vec());
        assert_eq!(rows[0].columns(), ["held"]);
    }

    #[test]
    fn a_transaction_that_is_dropped_leaves_nothing_behind() {
        let mut store = memory();
        store.execute_batch("CREATE TABLE observations(id INTEGER PRIMARY KEY);").unwrap();
        {
            let mut tx = store.transaction().unwrap();
            tx.execute("INSERT INTO observations(id) VALUES(?)", &[Value::from(1_i64)]).unwrap();
        }
        assert!(store.query("SELECT id FROM observations", &[]).unwrap().is_empty());

        let mut tx = store.transaction().unwrap();
        tx.execute("INSERT INTO observations(id) VALUES(?)", &[Value::from(2_i64)]).unwrap();
        tx.commit().unwrap();
        assert_eq!(store.query("SELECT id FROM observations", &[]).unwrap()[0].integer(0).unwrap(), 2);
    }

    #[test]
    fn the_bootstrap_schema_is_installed_once_and_read_back() {
        let mut store = memory();
        bootstrap(&mut store, "node", BOOTSTRAP_VERSION).unwrap();
        bootstrap(&mut store, "node", BOOTSTRAP_VERSION).unwrap();
        assert_eq!(schema_version(&mut store, "node").unwrap(), Some(BOOTSTRAP_VERSION));
        assert_eq!(schema_version(&mut store, "manager").unwrap(), None);
        assert_eq!(store.tables().unwrap(), ["store_schema"]);
        assert_eq!(store.query("SELECT COUNT(*) FROM store_schema", &[]).unwrap()[0].integer(0).unwrap(), 1);
    }

    #[test]
    fn a_missing_table_is_a_schema_fault_and_a_broken_key_an_integrity_one() {
        let mut store = memory();
        assert_eq!(store.query("SELECT value FROM metadata", &[]).unwrap_err().fault, Fault::Schema);
        store.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        store.execute("INSERT INTO metadata VALUES(?, ?)", &[Value::from("machine_id"), Value::from("x")]).unwrap();
        let again = store.execute("INSERT INTO metadata VALUES(?, ?)", &[Value::from("machine_id"), Value::from("y")]);
        assert_eq!(again.unwrap_err().fault, Fault::Integrity);
    }

    #[test]
    fn a_store_another_writer_holds_is_busy_not_failed() {
        let dir = scratch("busy");
        let config = SqliteConfig { path: dir.join("state.sqlite"), busy_timeout: std::time::Duration::ZERO, ..SqliteConfig::default() };
        let mut held = SqliteStore::open(&config).unwrap();
        held.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        let mut waiting = SqliteStore::open(&config).unwrap();

        let mut writing = held.transaction().unwrap();
        writing.execute("INSERT INTO metadata VALUES(?, ?)", &[Value::from("host_uuid"), Value::from("held")]).unwrap();
        let refused = waiting
            .execute("INSERT INTO metadata VALUES(?, ?)", &[Value::from("host_uuid"), Value::from("waiting")])
            .unwrap_err();
        assert!(refused.retryable(), "a held store answers busy or locked, said {refused}");
        writing.commit().unwrap();

        // Once the writer is gone the same statement is accepted, which is what retryable means.
        waiting.execute("INSERT INTO metadata VALUES(?, ?)", &[Value::from("cluster"), Value::from("waiting")]).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_intact_store_checks_out() {
        let mut store = memory();
        bootstrap(&mut store, "node", BOOTSTRAP_VERSION).unwrap();
        let integrity = store.integrity_check().unwrap();
        assert!(integrity.ok && integrity.findings.is_empty());
        assert_eq!(integrity.to_json()["store_integrity_result"], "ok");
        assert_eq!(integrity.to_json()["engine"], "sqlite");
    }

    /// The adapter proof of this phase: the journal a node opens today, written through the
    /// contract, is the same file `rusqlite` reads back -- same path, same tables, same rows.
    /// No caller is moved here; this only shows the layer sits where `open_state` sits.
    #[test]
    fn the_journal_of_a_node_is_the_same_file_through_the_store() {
        let dir = scratch("journal");
        let config = super::super::StoreConfig::for_state_dir(&dir);
        assert_eq!(config.sqlite.path, dir.join("state.sqlite"));

        let mut store = SqliteStore::open(&config.sqlite).unwrap();
        store
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); \
                 CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,observed_at INTEGER NOT NULL,operation TEXT NOT NULL,result TEXT NOT NULL);",
            )
            .unwrap();
        let mut tx = store.transaction().unwrap();
        tx.execute("INSERT INTO metadata VALUES(?, ?)", &[Value::from("host_uuid"), Value::from("9c1b")]).unwrap();
        tx.commit().unwrap();
        assert_eq!(store.tables().unwrap(), ["metadata", "observations"]);
        drop(store);

        let db = Connection::open(dir.join("state.sqlite")).unwrap();
        let held: String = db.query_row("SELECT value FROM metadata WHERE key='host_uuid'", [], |row| row.get(0)).unwrap();
        assert_eq!(held, "9c1b");

        // And the other way: a connection the node already holds answers the contract as it is.
        let mut adopted = SqliteStore::adopt(db);
        assert_eq!(adopted.query("SELECT value FROM metadata WHERE key = ?", &[Value::from("host_uuid")]).unwrap()[0].text(0).unwrap(), "9c1b");
        fs::remove_dir_all(&dir).unwrap();
    }
}
