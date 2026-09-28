//! The MariaDB backend: the engine the storage plan moves the node's journal to.
//!
//! It is the synchronous client, on one connection, because the node is synchronous: one
//! operation at a time, in order, on the journal it owns. A pool would say that several writers
//! share this store, and the plan says the opposite -- one MariaDB instance per functional role,
//! the node's journal never shared with the manager's.
//!
//! Two things are set on every connection, and they are the reason this backend exists rather
//! than a bare client: InnoDB's row lock wait becomes [`Fault::Busy`] after the profile's
//! timeout instead of the server's minute-long default, and every error code the server returns
//! is mapped to the same named faults SQLite's codes map to, so a caller written for one engine
//! decides the same way on the other.
//!
//! The node's own schema installs through this backend as of Phase 2: [`super::migrations`] has
//! a MariaDB file for every migration, and `open_state` applies them when the profile names this
//! engine. What a node does **not** yet do is read and write its journal here -- its operations
//! still take a `rusqlite::Connection`, so a node configured for MariaDB migrates and is then
//! refused by name.
use super::config::{redact, Engine, MariadbConfig};
use super::{DurableStore, Fault, Integrity, Result, Row, Transaction, Value};
use mysql::prelude::Queryable;
use mysql::{Conn, DriverError, Opts, OptsBuilder, Params, TxOpts, Value as MyValue};
use std::sync::Arc;

/// MariaDB's collation number for "these bytes are not text"; a column that carries it is read
/// back as [`Value::Blob`] rather than decoded.
const BINARY_COLLATION: u16 = 63;

fn flush_log_is_one(value: &Value) -> bool {
    match value {
        Value::Integer(1) => true,
        Value::Text(text) => text == "1",
        Value::Real(f) => *f == 1.0,
        _ => false,
    }
}

fn require_flush_log_at_commit(conn: &mut Conn, target: &str) -> Result<()> {
    // MariaDB 10.11 exposes this as GLOBAL-only (unlike variables that may be
    // overridden per session), so asking for @@SESSION fails with error 1238.
    // The global value is the effective value for every transaction here.
    let rows = query_on(conn, "SELECT @@GLOBAL.innodb_flush_log_at_trx_commit", &[])?;
    let Some(row) = rows.into_iter().next() else {
        return Err(Fault::Other.error(format!(
            "{target} refuses to open: innodb_flush_log_at_trx_commit was unreadable"
        )));
    };
    let global = row.value(0)?;
    if flush_log_is_one(global) {
        return Ok(());
    }
    Err(Fault::Other.error(format!(
        "{target} refuses to open: global innodb_flush_log_at_trx_commit must be 1 \
         (global={global:?}); durability is not optional"
    )))
}

pub struct MariadbStore {
    conn: Conn,
    /// What this store is, without its password, for a message or a refusal.
    target: String,
}

impl MariadbStore {
    pub fn open(config: &MariadbConfig) -> Result<Self> {
        let target = config.described();
        let url = config.url()?;
        let opts = Opts::from_url(&url).map_err(|e| {
            Fault::Other.error(format!("{} is not a usable DSN: {e}", redact(&url)))
        })?;
        // Only the connect timeout is set here. A read timeout would end a long statement the
        // node meant to run, which is a different decision and not this layer's to take.
        let opts = Opts::from(
            OptsBuilder::from_opts(opts).tcp_connect_timeout(Some(config.connect_timeout)),
        );
        let mut conn = Conn::new(opts)
            .map_err(|e| fault_of(&e).error(format!("{target} could not be opened: {e}")))?;
        let seconds = config.lock_wait_timeout.as_secs().max(1);
        conn.query_drop(format!("SET SESSION innodb_lock_wait_timeout = {seconds}"))
            .map_err(|e| {
                fault_of(&e).error(format!("{target} refused the lock wait timeout: {e}"))
            })?;
        // Muse must-fix: name the isolation the plan relies on, do not inherit a silent change.
        conn.query_drop("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .map_err(|e| fault_of(&e).error(format!("{target} refused REPEATABLE READ: {e}")))?;
        // Muse must-fix: refuse a server that does not flush the redo log on every commit.
        // Session and global are both checked so a SESSION override cannot hide a GLOBAL=0/2.
        require_flush_log_at_commit(&mut conn, &target)?;
        Ok(Self { conn, target })
    }

    /// What this store is, without its password.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The database this connection is in, which is where every unqualified name resolves.
    pub fn database(&mut self) -> Result<Option<String>> {
        let row = query_on(&mut self.conn, "SELECT DATABASE()", &[])?
            .into_iter()
            .next();
        Ok(row.and_then(|row| row.value(0).ok().and_then(Value::text).map(str::to_string)))
    }
}

impl DurableStore for MariadbStore {
    fn engine(&self) -> Engine {
        Engine::Mariadb
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        execute_on(&mut self.conn, sql, params)
    }

    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        // The server takes one statement per call unless multi-statement is on, and it is not:
        // a batch that arrives as one string is a batch the server would parse as one statement.
        for statement in statements(sql) {
            self.conn
                .query_drop(&statement)
                .map_err(|e| fault_of(&e).error(format!("{statement}: {e}")))?;
        }
        Ok(())
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        query_on(&mut self.conn, sql, params)
    }

    fn transaction(&mut self) -> Result<Box<dyn Transaction + '_>> {
        // The server's default isolation, which the plan names: REPEATABLE READ.
        let tx = self
            .conn
            .start_transaction(TxOpts::default())
            .map_err(|e| fault_of(&e).error(format!("a transaction could not be opened: {e}")))?;
        Ok(Box::new(MariadbTransaction { tx }))
    }

    fn integrity_check(&mut self) -> Result<Integrity> {
        let checked = self.tables()?;
        let mut findings = Vec::new();
        for table in &checked {
            let rows = query_on(
                &mut self.conn,
                &format!("CHECK TABLE {}", quoted(table)?),
                &[],
            )?;
            for row in rows {
                let kind = row
                    .named("Msg_type")
                    .or_else(|_| row.value(2))?
                    .text()
                    .unwrap_or_default()
                    .to_string();
                let said = row
                    .named("Msg_text")
                    .or_else(|_| row.value(3))?
                    .text()
                    .unwrap_or_default()
                    .to_string();
                if !(kind == "status" && said == "OK") {
                    findings.push(format!("{table}: {kind}: {said}"));
                }
            }
        }
        Ok(Integrity {
            engine: Engine::Mariadb,
            ok: findings.is_empty(),
            checked,
            findings,
        })
    }

    fn tables(&mut self) -> Result<Vec<String>> {
        let rows = query_on(
            &mut self.conn,
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' ORDER BY table_name",
            &[],
        )?;
        rows.iter()
            .map(|row| row.text(0).map(str::to_string))
            .collect()
    }
}

struct MariadbTransaction<'a> {
    tx: mysql::Transaction<'a>,
}

impl Transaction for MariadbTransaction<'_> {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        execute_on(&mut self.tx, sql, params)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
        query_on(&mut self.tx, sql, params)
    }

    fn commit(self: Box<Self>) -> Result<()> {
        self.tx
            .commit()
            .map_err(|e| fault_of(&e).error(format!("the transaction could not be committed: {e}")))
    }

    fn rollback(self: Box<Self>) -> Result<()> {
        self.tx.rollback().map_err(|e| {
            fault_of(&e).error(format!("the transaction could not be rolled back: {e}"))
        })
    }
}

/// What a connection and a transaction both answer: how many rows the last statement changed.
trait Executor: Queryable {
    fn rows_affected(&self) -> u64;
}

impl Executor for Conn {
    fn rows_affected(&self) -> u64 {
        self.affected_rows()
    }
}

impl Executor for mysql::Transaction<'_> {
    fn rows_affected(&self) -> u64 {
        self.affected_rows()
    }
}

fn execute_on<E: Executor>(executor: &mut E, sql: &str, params: &[Value]) -> Result<u64> {
    executor
        .exec_drop(sql, params_of(params))
        .map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))?;
    Ok(executor.rows_affected())
}

fn query_on<Q: Queryable>(queryable: &mut Q, sql: &str, params: &[Value]) -> Result<Vec<Row>> {
    let rows: Vec<mysql::Row> = queryable
        .exec(sql, params_of(params))
        .map_err(|e| fault_of(&e).error(format!("{sql}: {e}")))?;
    let mut columns: Option<Arc<Vec<String>>> = None;
    let mut binary: Vec<bool> = Vec::new();
    let mut collected = Vec::with_capacity(rows.len());
    for row in rows {
        if columns.is_none() {
            columns = Some(Arc::new(
                row.columns_ref()
                    .iter()
                    .map(|column| column.name_str().to_string())
                    .collect(),
            ));
            binary = row
                .columns_ref()
                .iter()
                .map(|column| column.character_set() == BINARY_COLLATION)
                .collect();
        }
        let values = row
            .unwrap_raw()
            .into_iter()
            .enumerate()
            .map(|(index, value)| match value {
                Some(value) => from_mysql(value, binary.get(index).copied().unwrap_or(false)),
                None => Value::Null,
            })
            .collect();
        collected.push(Row::new(columns.clone().unwrap_or_default(), values));
    }
    Ok(collected)
}

fn params_of(params: &[Value]) -> Params {
    if params.is_empty() {
        return Params::Empty;
    }
    Params::Positional(params.iter().map(to_mysql).collect())
}

fn to_mysql(value: &Value) -> MyValue {
    match value {
        Value::Null => MyValue::NULL,
        Value::Integer(i) => MyValue::Int(*i),
        Value::Real(f) => MyValue::Double(*f),
        Value::Text(text) => MyValue::Bytes(text.as_bytes().to_vec()),
        Value::Blob(bytes) => MyValue::Bytes(bytes.clone()),
    }
}

/// A server value as one of the five kinds. Bytes are text unless the column says they are not,
/// or unless they are not text at all: a caller reading a `VARBINARY` gets its bytes back.
fn from_mysql(value: MyValue, binary: bool) -> Value {
    match value {
        MyValue::NULL => Value::Null,
        MyValue::Int(i) => Value::Integer(i),
        MyValue::UInt(u) => {
            i64::try_from(u).map_or_else(|_| Value::Text(u.to_string()), Value::Integer)
        }
        MyValue::Float(f) => Value::Real(f64::from(f)),
        MyValue::Double(f) => Value::Real(f),
        MyValue::Bytes(bytes) if binary => Value::Blob(bytes),
        MyValue::Bytes(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Value::Text(text),
            Err(not_text) => Value::Blob(not_text.into_bytes()),
        },
        MyValue::Date(year, month, day, hour, minute, second, micros) => Value::Text(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}"
        )),
        MyValue::Time(negative, days, hours, minutes, seconds, micros) => Value::Text(format!(
            "{}{:02}:{minutes:02}:{seconds:02}.{micros:06}",
            if negative { "-" } else { "" },
            u32::from(hours) + days * 24
        )),
    }
}

/// A name this store may put between backticks. Anything else is refused rather than quoted:
/// a table name is read from `information_schema`, never from a caller's string.
fn quoted(identifier: &str) -> Result<String> {
    let plain = !identifier.is_empty()
        && identifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '$'));
    if !plain {
        return Err(Fault::Schema.error(format!("{identifier} is not a name this store quotes")));
    }
    Ok(format!("`{identifier}`"))
}

/// The statements a batch carries, split on the semicolons that end them -- not on the ones
/// inside a quoted string, a quoted name or a comment.
fn statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut comment = false;
    let mut characters = sql.chars().peekable();
    while let Some(c) = characters.next() {
        if comment {
            if c == '\n' {
                comment = false;
                current.push(c);
            }
            continue;
        }
        match quote {
            Some(open) => {
                current.push(c);
                if c == '\\' {
                    if let Some(escaped) = characters.next() {
                        current.push(escaped);
                    }
                } else if c == open {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' => {
                    quote = Some(c);
                    current.push(c);
                }
                '#' => comment = true,
                '-' if characters.peek() == Some(&'-') => {
                    characters.next();
                    comment = true;
                }
                ';' => {
                    if !current.trim().is_empty() {
                        statements.push(current.trim().to_string());
                    }
                    current.clear();
                }
                _ => current.push(c),
            },
        }
    }
    if !current.trim().is_empty() {
        statements.push(current.trim().to_string());
    }
    statements
}

/// The server's error codes as the faults a caller decides on. 1205 is a row lock that waited
/// past its timeout and 1213 a deadlock InnoDB broke: both say "this attempt, later", which is
/// what `SQLITE_BUSY` and `SQLITE_LOCKED` say on the other engine.
fn fault_of(error: &mysql::Error) -> Fault {
    match error {
        mysql::Error::MySqlError(said) => from_code(said.code),
        mysql::Error::IoError(_) | mysql::Error::CodecError(_) => Fault::Unavailable,
        mysql::Error::DriverError(
            DriverError::ConnectTimeout | DriverError::CouldNotConnect(_) | DriverError::Timeout,
        ) => Fault::Unavailable,
        _ => Fault::Other,
    }
}

fn from_code(code: u16) -> Fault {
    match code {
        1205 => Fault::Busy,                               // lock wait timeout exceeded
        1040 | 1203 => Fault::Busy, // too many connections, for this user or at all
        1213 => Fault::Locked,      // deadlock, broken by the server
        1044 | 1045 | 1142 | 1143 | 1227 => Fault::Denied, // access, privilege, super
        1046 | 1049 | 1051 | 1054 | 1109 | 1146 => Fault::Schema, // no database, table, column selected or found
        1022 | 1048 | 1062 | 1451 | 1452 | 1557 => Fault::Integrity, // key, null, duplicate, foreign key
        1264 | 1366 | 1406 => Fault::Type, // out of range, wrong value, too long
        _ => Fault::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::DSN_ENVIRONMENT;
    use super::super::{bootstrap, schema_version, BOOTSTRAP_VERSION};
    use super::*;

    /// One MariaDB test at a time: they all write into the one database the DSN names, and each
    /// drops the tables it made. A poisoned lock is taken anyway -- the test that panicked has
    /// already reported, and holding the rest back would report it again as a failure of theirs.
    fn serialized() -> std::sync::MutexGuard<'static, ()> {
        super::super::MARIADB_TEST_SERVER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The store the environment names, or none. A DSN that is set and does not open is a
    /// failure, not a skip: an operator who names a server is asking for it to be used.
    fn server() -> Option<MariadbStore> {
        let config = MariadbConfig::from_environment()?;
        match MariadbStore::open(&config) {
            Ok(store) => Some(store),
            Err(error) => panic!(
                "{DSN_ENVIRONMENT} names {}, which did not open: {error}",
                config.described()
            ),
        }
    }

    /// Take the server, then open it: `store_or_skip!(store)` declares the guard and the store,
    /// and returns from the test when no server is named.
    macro_rules! store_or_skip {
        ($store:ident) => {
            let _serialized = serialized();
            #[allow(unused_mut)]
            let mut $store = match server() {
                Some(store) => store,
                None => {
                    eprintln!("skipped: {DSN_ENVIRONMENT} names no MariaDB server");
                    return;
                }
            };
        };
    }

    /// Phase 1's exit criterion for this engine: an empty MariaDB instance carries the store's
    /// bootstrap schema, records a version in a transaction, reads it back, lists what it holds
    /// and checks out. Nothing of the node's own schema is installed; that is Phase 2.
    #[test]
    fn a_mariadb_instance_carries_the_bootstrap_schema() {
        store_or_skip!(store);
        assert_eq!(store.engine(), Engine::Mariadb);
        assert!(
            store.database().unwrap().is_some(),
            "the DSN names no database to write in"
        );

        store
            .execute_batch("DROP TABLE IF EXISTS store_schema;")
            .unwrap();
        bootstrap(&mut store, "node", BOOTSTRAP_VERSION).unwrap();
        // Run again: a bootstrap is what a node does at every start, not once ever.
        bootstrap(&mut store, "node", BOOTSTRAP_VERSION).unwrap();

        assert_eq!(
            schema_version(&mut store, "node").unwrap(),
            Some(BOOTSTRAP_VERSION)
        );
        assert_eq!(schema_version(&mut store, "manager").unwrap(), None);
        assert_eq!(
            store
                .query("SELECT COUNT(*) FROM store_schema", &[])
                .unwrap()[0]
                .integer(0)
                .unwrap(),
            1
        );
        assert!(store
            .tables()
            .unwrap()
            .iter()
            .any(|table| table == "store_schema"));

        let integrity = store.integrity_check().unwrap();
        assert!(integrity.ok, "{:?}", integrity.findings);
        assert_eq!(integrity.to_json()["store_integrity_result"], "ok");
        assert_eq!(integrity.to_json()["engine"], "mariadb");

        store
            .execute_batch("DROP TABLE IF EXISTS store_schema;")
            .unwrap();
    }

    #[test]
    fn the_five_value_kinds_come_back_as_they_were_written() {
        store_or_skip!(store);
        store
            .execute_batch(
                "DROP TABLE IF EXISTS store_kinds; \
                 CREATE TABLE store_kinds(rank INT NOT NULL PRIMARY KEY, as_null INT NULL, as_integer BIGINT NULL, \
                 as_real DOUBLE NULL, as_text VARCHAR(64) NULL, as_blob VARBINARY(64) NULL) \
                 ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;",
            )
            .unwrap();
        let written = [
            Value::Null,
            Value::Integer(-7),
            Value::Real(0.5),
            Value::Text("a universe".into()),
            Value::Blob(vec![0, 159, 146, 150]),
        ];
        let changed = store
            .execute(
                "INSERT INTO store_kinds(rank, as_null, as_integer, as_real, as_text, as_blob) VALUES(?, ?, ?, ?, ?, ?)",
                &[
                    Value::from(1_i64),
                    written[0].clone(),
                    written[1].clone(),
                    written[2].clone(),
                    written[3].clone(),
                    written[4].clone(),
                ],
            )
            .unwrap();
        assert_eq!(changed, 1);

        let rows = store.query("SELECT as_null, as_integer, as_real, as_text, as_blob FROM store_kinds WHERE rank = ?", &[Value::from(1_i64)]).unwrap();
        let read: Vec<Value> = rows[0].values().to_vec();
        assert_eq!(read, written.to_vec());
        assert_eq!(rows[0].named("as_text").unwrap().text(), Some("a universe"));
        store
            .execute_batch("DROP TABLE IF EXISTS store_kinds;")
            .unwrap();
    }

    #[test]
    fn a_transaction_that_is_dropped_leaves_nothing_behind() {
        store_or_skip!(store);
        store
            .execute_batch(
                "DROP TABLE IF EXISTS store_observations; \
                 CREATE TABLE store_observations(id BIGINT NOT NULL PRIMARY KEY) ENGINE=InnoDB;",
            )
            .unwrap();
        {
            let mut tx = store.transaction().unwrap();
            tx.execute(
                "INSERT INTO store_observations(id) VALUES(?)",
                &[Value::from(1_i64)],
            )
            .unwrap();
        }
        assert!(store
            .query("SELECT id FROM store_observations", &[])
            .unwrap()
            .is_empty());

        let mut tx = store.transaction().unwrap();
        tx.execute(
            "INSERT INTO store_observations(id) VALUES(?)",
            &[Value::from(2_i64)],
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            store
                .query("SELECT id FROM store_observations", &[])
                .unwrap()[0]
                .integer(0)
                .unwrap(),
            2
        );

        let duplicated = store.execute(
            "INSERT INTO store_observations(id) VALUES(?)",
            &[Value::from(2_i64)],
        );
        assert_eq!(duplicated.unwrap_err().fault, Fault::Integrity);
        store
            .execute_batch("DROP TABLE IF EXISTS store_observations;")
            .unwrap();
    }

    /// The other half of the busy mapping, against the server rather than the code table: a row
    /// another transaction holds makes this one wait its profile's timeout, and what comes back
    /// is the same retryable fault SQLite gives for a file another writer holds.
    #[test]
    fn a_row_another_transaction_holds_is_busy_not_failed() {
        let _serialized = serialized();
        let Some(config) = MariadbConfig::from_environment() else {
            eprintln!("skipped: {DSN_ENVIRONMENT} names no MariaDB server");
            return;
        };
        let impatient = MariadbConfig {
            lock_wait_timeout: std::time::Duration::from_secs(1),
            ..config.clone()
        };
        let mut held = MariadbStore::open(&config).unwrap();
        held.execute_batch(
            "DROP TABLE IF EXISTS store_leases; \
             CREATE TABLE store_leases(id BIGINT NOT NULL PRIMARY KEY, holder VARCHAR(32) NOT NULL) ENGINE=InnoDB;",
        )
        .unwrap();
        held.execute(
            "INSERT INTO store_leases(id, holder) VALUES(?, ?)",
            &[Value::from(1_i64), Value::from("none")],
        )
        .unwrap();
        let mut waiting = MariadbStore::open(&impatient).unwrap();

        let mut writing = held.transaction().unwrap();
        writing
            .execute(
                "UPDATE store_leases SET holder = ? WHERE id = ?",
                &[Value::from("held"), Value::from(1_i64)],
            )
            .unwrap();
        let refused = waiting
            .execute(
                "UPDATE store_leases SET holder = ? WHERE id = ?",
                &[Value::from("waiting"), Value::from(1_i64)],
            )
            .unwrap_err();
        assert!(
            refused.retryable(),
            "a held row answers busy or locked, said {refused}"
        );
        writing.commit().unwrap();

        waiting
            .execute(
                "UPDATE store_leases SET holder = ? WHERE id = ?",
                &[Value::from("waiting"), Value::from(1_i64)],
            )
            .unwrap();
        held.execute_batch("DROP TABLE IF EXISTS store_leases;")
            .unwrap();
    }

    #[test]
    fn a_batch_is_split_on_the_semicolons_that_end_statements() {
        let split = statements(
            "CREATE TABLE a(held VARCHAR(8)); -- a comment; not a statement\n\
             INSERT INTO a VALUES('one;two'); # another; comment\n\
             INSERT INTO a VALUES(\"three;four\");",
        );
        assert_eq!(
            split,
            [
                "CREATE TABLE a(held VARCHAR(8))",
                "INSERT INTO a VALUES('one;two')",
                "INSERT INTO a VALUES(\"three;four\")",
            ]
        );
        assert!(statements("  ;  ; ").is_empty());
        assert_eq!(statements("SELECT 1").len(), 1);
    }

    #[test]
    fn the_codes_that_mean_later_are_the_retryable_ones() {
        assert!(from_code(1205).retryable() && from_code(1213).retryable());
        assert_eq!(from_code(1146), Fault::Schema);
        assert_eq!(from_code(1062), Fault::Integrity);
        assert_eq!(from_code(1045), Fault::Denied);
        assert!(!from_code(1064).retryable());
    }

    #[test]
    fn a_name_that_is_not_plain_is_refused_rather_than_quoted() {
        assert_eq!(quoted("store_schema").unwrap(), "`store_schema`");
        assert_eq!(quoted("podmesh-node").unwrap(), "`podmesh-node`");
        for refused in ["", "a`b", "a b", "a;b", "a\"b"] {
            assert_eq!(
                quoted(refused).unwrap_err().fault,
                Fault::Schema,
                "{refused}"
            );
        }
    }
}
