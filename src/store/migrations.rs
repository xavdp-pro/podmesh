//! The node's schema, as one ordered set of migrations instead of DDL scattered through modules.
//!
//! Until Phase 2 every table of `state.sqlite` was created lazily, by the module that reads it,
//! in that module's own `ensure_schema`. Nothing recorded which shape the journal was in, and the
//! only way to know what a store carried was to open it. That works while there is one engine and
//! one dialect. It does not survive a second engine: `TEXT PRIMARY KEY` is a key on SQLite and a
//! refusal on MariaDB, `INTEGER PRIMARY KEY` is a rowid alias on one and needs `AUTO_INCREMENT`
//! on the other, and a partial index exists on one engine only.
//!
//! So the schema lives here, in `migrations/node/`, as numbered files applied in order and
//! recorded in [`SCHEMA_TABLE`](super::SCHEMA_TABLE): version *N* means the first *N* migrations
//! of this set have been applied. Each migration is a pair of files, one per engine, because
//! every `CREATE TABLE` of this set diverges -- a variable-length primary key needs a length on
//! MariaDB and none on SQLite, so there is no portable spelling of even the first table. Where a
//! future migration is portable (adding an index, inserting a row), the same file may be named
//! for both engines through [`Migration::portable`].
//!
//! Two rules hold for every file in the set, and a new migration keeps them:
//!
//! * **Every statement is idempotent** (`CREATE TABLE IF NOT EXISTS`, `CREATE UNIQUE INDEX IF NOT
//!   EXISTS`). MariaDB commits each DDL statement on its own -- a migration is not a transaction
//!   there, whatever the engine promises elsewhere -- so a migration interrupted halfway is
//!   recovered by running it again, not by rolling it back.
//! * **A migration already released is never edited.** The node's schema moves forward by adding
//!   a file, because a store that recorded version *N* will never run the first *N* files again.
//!
//! The files reproduce the schema a node carries today, including the columns later versions
//! appended with `ALTER TABLE`, in the order they were appended. The modules keep their
//! `ensure_schema`: a journal written by an older package still needs those `ALTER TABLE`
//! upgrades, and `CREATE TABLE IF NOT EXISTS` adds no column to a table that already exists.
//! `the_migrations_carry_the_schema_the_modules_create` below is what keeps the two in step.
use super::config::Engine;
use super::{DurableStore, Fault, Result};

/// The schema name the node's set is recorded under in [`SCHEMA_TABLE`](super::SCHEMA_TABLE).
pub const NODE: &str = "node";

/// Offline SQLite→MariaDB cutover marker in [`SCHEMA_TABLE`](super::SCHEMA_TABLE).
/// While version is [`CUTOVER_INCOMPLETE`], the node must refuse to open the store.
pub const CUTOVER: &str = "node_cutover";

/// Recorded under [`CUTOVER`] until copy and verification finish.
pub const CUTOVER_INCOMPLETE: i64 = 0;

/// One step of a schema, in the dialect of each engine that carries it.
pub struct Migration {
    /// The file name without its engine and extension, e.g. `0004-activation`. Ordered by it.
    pub id: &'static str,
    sqlite: &'static str,
    mariadb: &'static str,
}

impl Migration {
    /// A step whose statements are the same on both engines.
    pub const fn portable(id: &'static str, sql: &'static str) -> Self {
        Self { id, sqlite: sql, mariadb: sql }
    }

    /// A step each engine spells its own way, which every `CREATE TABLE` of the node set is.
    pub const fn split(id: &'static str, sqlite: &'static str, mariadb: &'static str) -> Self {
        Self { id, sqlite, mariadb }
    }

    pub fn sql(&self, engine: Engine) -> &'static str {
        match engine {
            Engine::Sqlite => self.sqlite,
            Engine::Mariadb => self.mariadb,
        }
    }
}

macro_rules! node {
    ($id:literal) => {
        Migration::split(
            $id,
            include_str!(concat!("migrations/node/", $id, ".sqlite.sql")),
            include_str!(concat!("migrations/node/", $id, ".mariadb.sql")),
        )
    };
}

/// The node's schema, in order. The number in a file name is its version, and the set's length is
/// the version a store carries once every one of them has been applied.
pub const NODE_MIGRATIONS: &[Migration] = &[
    node!("0001-base"),
    node!("0002-lifecycle"),
    node!("0003-migration"),
    node!("0004-activation"),
    node!("0005-network"),
    node!("0006-secrets"),
    node!("0007-publisher"),
    node!("0008-recovery-point"),
    node!("0009-retention"),
    node!("0010-collector"),
];

/// The version a store carries once the whole node set has been applied.
pub fn node_version() -> i64 {
    NODE_MIGRATIONS.len() as i64
}

/// What one call to [`apply`] found and what it did.
#[derive(Clone, Debug)]
pub struct Applied {
    pub schema: &'static str,
    /// The version the store carried before; 0 for a store that never carried this schema.
    pub from: i64,
    /// The version it carries now.
    pub to: i64,
    /// The migrations this call ran, in order; empty when the store was already at `to`.
    pub ran: Vec<&'static str>,
}

impl Applied {
    /// One line for a startup message: what was found, and what was done about it.
    pub fn described(&self) -> String {
        if self.ran.is_empty() {
            format!("{} schema at version {}, nothing to apply", self.schema, self.to)
        } else {
            format!("{} schema {} -> {}: {}", self.schema, self.from, self.to, self.ran.join(", "))
        }
    }
}

/// Bring a store up to the node's current schema, and record which version it now carries.
pub fn apply(store: &mut dyn DurableStore) -> Result<Applied> {
    apply_set(store, NODE, NODE_MIGRATIONS)
}

/// The same, for a named set: the manager's store follows in Phase 3 with a set of its own.
///
/// A store recorded at a version this build does not have is **refused**, not downgraded: the
/// package was rolled back under a journal a later one wrote, and the later shape is the truth.
pub fn apply_set(store: &mut dyn DurableStore, schema: &'static str, set: &'static [Migration]) -> Result<Applied> {
    let engine = store.engine();
    let latest = set.len() as i64;
    super::ensure_schema_table(store)?;
    let from = super::schema_version(store, schema)?.unwrap_or(0);
    if from > latest {
        return Err(Fault::Schema.error(format!(
            "the store carries {schema} schema version {from} and this build knows {latest}: \
             a journal is never opened by a build older than the one that wrote it"
        )));
    }
    let mut ran = Vec::new();
    for (index, migration) in set.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= from {
            continue;
        }
        store.execute_batch(migration.sql(engine))?;
        // Recorded after the statements it names, and each of them may be run again: a crash
        // between the two leaves the store at the previous version and the next open repeats it.
        super::bootstrap(store, schema, version)?;
        ran.push(migration.id);
    }
    Ok(Applied { schema, from, to: latest, ran })
}

/// Every table the node's schema carries, in the order the set creates them. This is the list the
/// storage inventory names, and what a caller checks a store against without reading a catalog.
pub fn node_tables() -> Vec<&'static str> {
    node_tables_through(node_version())
}

/// Tables created by the first `version` migrations (1..=version). Version 0 is empty.
pub fn node_tables_through(version: i64) -> Vec<&'static str> {
    let mut tables = Vec::new();
    let limit = version.max(0) as usize;
    for migration in NODE_MIGRATIONS.iter().take(limit) {
        for statement in sql_statements(migration.sql(Engine::Sqlite)) {
            if let Some(name) = created_table(statement) {
                tables.push(name);
            }
        }
    }
    tables
}

/// Refuse a store whose offline cutover never finished.
pub fn refuse_incomplete_cutover(store: &mut dyn DurableStore) -> Result<()> {
    if super::schema_version(store, CUTOVER)? == Some(CUTOVER_INCOMPLETE) {
        return Err(Fault::Schema.error(
            "offline cutover is incomplete (store_schema.node_cutover = 0): \
             refuse to open; finish or re-run podmesh-storage-migrate with --force after a verified backup",
        ));
    }
    Ok(())
}

/// The table a `CREATE TABLE IF NOT EXISTS <name>(` statement creates, or none for anything else.
/// The migration files are this crate's own text, included at compile time, so the shape is known.
/// A leading `--` comment is skipped: the first table of each file is introduced that way.
fn created_table(statement: &'static str) -> Option<&'static str> {
    let mut words = strip_leading_line_comments(statement).split_whitespace();
    for want in ["CREATE", "TABLE", "IF", "NOT", "EXISTS"] {
        if !words.next().is_some_and(|word| word.eq_ignore_ascii_case(want)) {
            return None;
        }
    }
    words.next()?.split('(').next()
}

fn strip_leading_line_comments(statement: &str) -> &str {
    let mut rest = statement.trim_start();
    while let Some(body) = rest.strip_prefix("--") {
        rest = match body.find('\n') {
            Some(end) => body[end + 1..].trim_start(),
            None => "",
        };
    }
    rest
}

/// Split on statement-ending semicolons, not on the ones inside a `--` line comment or a quote.
/// Inventory comments name two tables in one sentence; a naive `split(';')` cuts the `CREATE` away.
fn sql_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let bytes = sql.as_bytes();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    let mut comment = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            index += 1;
            continue;
        }
        if let Some(open) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                comment = true;
                index += 2;
                continue;
            }
            b';' => {
                statements.push(&sql[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start < sql.len() {
        statements.push(&sql[start..]);
    }
    statements
}

#[cfg(test)]
mod tests {
    use super::super::{schema_version, sqlite::SqliteStore, Value};
    use super::*;
    use std::collections::BTreeMap;

    /// Every table the node's storage inventory names (`docs/STORAGE-MARIADB-NODE-INVENTORY.md`).
    /// The inventory counts 38 production tables; the set carries all of them.
    const INVENTORY: [&str; 38] = [
        "activation_epochs",
        "activation_lease_history",
        "activation_leases",
        "activation_policy",
        "activation_policy_changes",
        "collection_holds",
        "garbage_collection_effects",
        "garbage_collection_runs",
        "metadata",
        "migration_authorizations",
        "migration_collection_history",
        "migration_reservation_history",
        "migration_reservations",
        "migration_restore_claims",
        "migration_universe_tombstones",
        "network_allocations",
        "network_declaration",
        "network_effects",
        "network_peer_pools",
        "network_routes",
        "observations",
        "operation_attempts",
        "operations",
        "publisher_events",
        "publisher_takeover_verified",
        "publisher_transitions",
        "publishers",
        "recovery_point_final_captures",
        "recovery_point_live_captures",
        "recovery_point_live_promote_attempts",
        "recovery_point_live_promotions",
        "recovery_point_promotions",
        "recovery_point_restores",
        "recovery_point_retained",
        "recovery_point_retention",
        "recovery_point_staged",
        "recovery_points",
        "secrets",
    ];

    fn memory() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap()
    }

    /// The column names of every table, and the definition of every index, as SQLite reports
    /// them: enough to say that two journals have the same shape, whichever way it was made.
    fn shape(db: &rusqlite::Connection) -> BTreeMap<String, Vec<String>> {
        let mut shape = BTreeMap::new();
        let mut tables = db
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let names: Vec<String> = tables.query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        for name in names {
            let mut columns = db.prepare(&format!("PRAGMA table_info({name})")).unwrap();
            let described: Vec<String> = columns
                .query_map([], |row| {
                    Ok(format!(
                        "{} {} notnull={} default={:?} pk={}",
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            shape.insert(name, described);
        }
        let mut indexes = db
            .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL ORDER BY name")
            .unwrap();
        let described: Vec<String> = indexes
            .query_map([], |row| Ok(format!("{}: {}", row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        shape.insert("(indexes)".to_string(), described);
        shape
    }

    #[test]
    fn the_set_is_ordered_and_names_every_table_of_the_inventory() {
        let ids: Vec<&str> = NODE_MIGRATIONS.iter().map(|migration| migration.id).collect();
        let mut ordered = ids.clone();
        ordered.sort_unstable();
        assert_eq!(ids, ordered, "migrations are applied in the order of their names");
        assert_eq!(ids.len(), node_version() as usize);

        let mut tables = node_tables();
        tables.sort_unstable();
        let unique: std::collections::BTreeSet<&str> = tables.iter().copied().collect();
        assert_eq!(unique.len(), tables.len(), "a table is created by one migration only");
        assert_eq!(tables, INVENTORY, "the set carries the inventory's production tables");
    }

    #[test]
    fn an_empty_store_is_brought_to_the_current_version_once() {
        let mut store = memory();
        let applied = apply(&mut store).unwrap();
        assert_eq!((applied.from, applied.to), (0, node_version()));
        assert_eq!(applied.ran.len(), NODE_MIGRATIONS.len());
        assert!(applied.described().contains("0001-base"));

        let mut carried = store.tables().unwrap();
        carried.retain(|table| table != super::super::SCHEMA_TABLE);
        assert_eq!(carried, INVENTORY);
        assert_eq!(schema_version(&mut store, NODE).unwrap(), Some(node_version()));

        // Run again, as a node does at every start: nothing is applied and nothing is lost.
        store.execute("INSERT INTO metadata(`key`, value) VALUES(?, ?)", &[Value::from("host_uuid"), Value::from("9c1b")]).unwrap();
        let again = apply(&mut store).unwrap();
        assert!(again.ran.is_empty());
        assert_eq!(again.described(), format!("node schema at version {}, nothing to apply", node_version()));
        assert_eq!(store.query("SELECT value FROM metadata", &[]).unwrap()[0].text(0).unwrap(), "9c1b");
    }

    /// The proof that moves the DDL without changing it: a journal made by the migrations has the
    /// same tables, columns, defaults, keys and indexes as a journal made by every module's
    /// `ensure_schema`, which is what every node written before Phase 2 carries.
    #[test]
    fn the_migrations_carry_the_schema_the_modules_create() {
        let mut migrated = memory();
        apply(&mut migrated).unwrap();
        let migrated = migrated.into_connection();

        let lazy = rusqlite::Connection::open_in_memory().unwrap();
        lazy.execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); \
             CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,observed_at INTEGER NOT NULL,operation TEXT NOT NULL,result TEXT NOT NULL);",
        )
        .unwrap();
        crate::lifecycle::ensure_schema(&lazy).unwrap();
        crate::activation::ensure_schema(&lazy).unwrap();
        crate::network::ensure_schema(&lazy).unwrap();
        crate::secrets::ensure_schema(&lazy).unwrap();
        crate::publisher::ensure_schema(&lazy).unwrap();
        crate::recovery_point::ensure_schema(&lazy).unwrap();
        crate::retention::ensure_schema(&lazy).unwrap();
        crate::collector::ensure_schema(&lazy).unwrap();

        let mut made_by_migrations = shape(&migrated);
        made_by_migrations.remove(super::super::SCHEMA_TABLE);
        let made_by_modules = shape(&lazy);
        for (table, columns) in &made_by_modules {
            assert_eq!(made_by_migrations.get(table), Some(columns), "{table} differs");
        }
        assert_eq!(made_by_migrations.keys().collect::<Vec<_>>(), made_by_modules.keys().collect::<Vec<_>>());
    }

    /// And the modules add nothing to a journal the migrations made: every `ALTER TABLE` upgrade
    /// they carry finds its column already there, so an open runs them for nothing and no journal
    /// is rewritten behind the migration set.
    #[test]
    fn the_modules_change_nothing_in_a_migrated_journal() {
        let mut store = memory();
        apply(&mut store).unwrap();
        let db = store.into_connection();
        let before = shape(&db);
        crate::lifecycle::ensure_schema(&db).unwrap();
        crate::activation::ensure_schema(&db).unwrap();
        crate::network::ensure_schema(&db).unwrap();
        crate::secrets::ensure_schema(&db).unwrap();
        crate::publisher::ensure_schema(&db).unwrap();
        crate::recovery_point::ensure_schema(&db).unwrap();
        crate::retention::ensure_schema(&db).unwrap();
        crate::collector::ensure_schema(&db).unwrap();
        assert_eq!(shape(&db), before);
    }

    /// The partial unique indexes are the rule the two dialects state differently, so the rule
    /// itself is tested rather than the spelling: one live allocation per address and per
    /// universe, and a released row that takes part in neither.
    #[test]
    fn only_a_live_allocation_is_unique() {
        let mut store = memory();
        apply(&mut store).unwrap();
        let allocate = "INSERT INTO network_allocations(universe_uuid, network_uuid, ip, allocated_at, operation_id, released_at, released_by) \
                        VALUES(?, ?, ?, 1, 'op', ?, ?)";
        let row = |universe: &str, ip: &str, released: Option<i64>| {
            vec![
                Value::from(universe),
                Value::from("net"),
                Value::from(ip),
                Value::from(released),
                Value::from(released.map(|_| "op")),
            ]
        };
        store.execute(allocate, &row("u1", "10.0.0.1", None)).unwrap();
        assert_eq!(store.execute(allocate, &row("u2", "10.0.0.1", None)).unwrap_err().fault, Fault::Integrity);
        assert_eq!(store.execute(allocate, &row("u1", "10.0.0.2", None)).unwrap_err().fault, Fault::Integrity);

        // Released, the address and the universe are free again, and the history stays.
        store.execute("UPDATE network_allocations SET released_at = 2, released_by = 'op' WHERE ip = ?", &[Value::from("10.0.0.1")]).unwrap();
        store.execute(allocate, &row("u2", "10.0.0.1", None)).unwrap();
        store.execute(allocate, &row("u1", "10.0.0.3", None)).unwrap();
        assert_eq!(store.query("SELECT ip FROM network_allocations", &[]).unwrap().len(), 3);
    }

    /// Phase 2's exit criterion for the other engine: the node's whole schema installs on a real
    /// MariaDB instance, records its version, is not re-applied on the next open, and enforces the
    /// live-allocation rule the SQLite partial indexes state -- through a virtual column here,
    /// which is the one construct of the set the two dialects cannot share.
    ///
    /// It also asserts what Phase 1 put on the open path and Phase 2 must not have moved off it:
    /// the store refused to open unless the server flushes its redo log on every commit.
    ///
    /// The instance is named by `PODMESH_MARIADB_DSN` and is written into: the test drops the
    /// node's tables before and after. Point it at a store of its own, never at one a node uses.
    #[cfg(feature = "mariadb")]
    #[test]
    fn a_mariadb_instance_carries_the_whole_node_schema() {
        use super::super::{config::DSN_ENVIRONMENT, mariadb::MariadbStore, MariadbConfig};

        let _serialized = super::super::MARIADB_TEST_SERVER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(config) = MariadbConfig::from_environment() else {
            eprintln!("skipped: {DSN_ENVIRONMENT} names no MariaDB server");
            return;
        };
        config.validate().expect("the DSN names a database to migrate in");
        let mut store = match MariadbStore::open(&config) {
            Ok(store) => store,
            Err(error) => panic!("{DSN_ENVIRONMENT} names {}, which did not open: {error}", config.described()),
        };

        // Phase 1's durability gate is still what let this open succeed.
        let flush = store.query("SELECT @@GLOBAL.innodb_flush_log_at_trx_commit", &[]).unwrap();
        assert_eq!(flush[0].value(0).unwrap().clone(), Value::Integer(1), "the open gate let a non-flushing server through");

        let drop_everything = |store: &mut MariadbStore| {
            for table in INVENTORY.iter().chain([super::super::SCHEMA_TABLE].iter()) {
                store.execute_batch(&format!("DROP TABLE IF EXISTS `{table}`;")).unwrap();
            }
        };
        drop_everything(&mut store);

        let applied = apply(&mut store).unwrap();
        assert_eq!((applied.from, applied.to), (0, node_version()));
        assert_eq!(applied.ran.len(), NODE_MIGRATIONS.len());
        let mut carried = store.tables().unwrap();
        carried.retain(|table| table != super::super::SCHEMA_TABLE);
        // Sorted here rather than taken as the store returned it: `ORDER BY` follows the server's
        // collation, and where an underscore falls against a letter is not the same on the two
        // engines. What the migration set promises is the same tables, not the same order.
        carried.sort();
        assert_eq!(carried, INVENTORY, "the instance carries the node's tables and nothing else");
        assert_eq!(schema_version(&mut store, NODE).unwrap(), Some(node_version()));

        // A node's own statements read the same on this engine: the reserved names are quoted the
        // one way both engines accept, and a row written in a transaction is read back after it.
        let mut tx = store.transaction().unwrap();
        tx.execute("INSERT INTO metadata(`key`, value) VALUES(?, ?)", &[Value::from("host_uuid"), Value::from("9c1b")]).unwrap();
        tx.commit().unwrap();
        assert_eq!(store.query("SELECT value FROM metadata", &[]).unwrap()[0].text(0).unwrap(), "9c1b");

        let again = apply(&mut store).unwrap();
        assert!(again.ran.is_empty(), "a second open applies nothing");
        assert_eq!(store.query("SELECT value FROM metadata", &[]).unwrap().len(), 1);

        // The rule the partial indexes carry on SQLite, checked on the engine that has none.
        let allocate = "INSERT INTO network_allocations(universe_uuid, network_uuid, ip, allocated_at, operation_id, released_at, released_by) \
                        VALUES(?, ?, ?, 1, 'op', ?, ?)";
        let row = |universe: &str, ip: &str, released: Option<i64>| {
            vec![
                Value::from(universe),
                Value::from("net"),
                Value::from(ip),
                Value::from(released),
                Value::from(released.map(|_| "op")),
            ]
        };
        store.execute(allocate, &row("u1", "10.0.0.1", None)).unwrap();
        assert_eq!(store.execute(allocate, &row("u2", "10.0.0.1", None)).unwrap_err().fault, Fault::Integrity);
        assert_eq!(store.execute(allocate, &row("u1", "10.0.0.2", None)).unwrap_err().fault, Fault::Integrity);
        store.execute("UPDATE network_allocations SET released_at = 2, released_by = 'op' WHERE ip = ?", &[Value::from("10.0.0.1")]).unwrap();
        store.execute(allocate, &row("u2", "10.0.0.1", None)).unwrap();
        assert_eq!(store.query("SELECT ip FROM network_allocations", &[]).unwrap().len(), 2);

        // And what the caller asks the catalog is answered without a dialect on this engine too.
        assert!(super::super::catalog::has_column(&mut store, "secrets", "state").unwrap());
        assert!(!super::super::catalog::has_column(&mut store, "secrets", "content").unwrap());
        assert_eq!(
            super::super::catalog::columns(&mut store, "recovery_point_staged").unwrap().last().map(String::as_str),
            Some("uncompressed_bytes")
        );

        let integrity = store.integrity_check().unwrap();
        assert!(integrity.ok, "{:?}", integrity.findings);
        drop_everything(&mut store);
    }

    #[test]
    fn a_store_from_a_later_build_is_refused_rather_than_downgraded() {
        let mut store = memory();
        super::super::ensure_schema_table(&mut store).unwrap();
        super::super::bootstrap(&mut store, NODE, node_version() + 1).unwrap();
        let refused = apply(&mut store).unwrap_err();
        assert_eq!(refused.fault, Fault::Schema);
        assert!(refused.message.contains("older than the one that wrote it"));
    }

    #[test]
    fn node_tables_through_follows_the_migration_prefix() {
        assert!(node_tables_through(0).is_empty());
        assert_eq!(node_tables_through(node_version()), node_tables());
        let first = node_tables_through(1);
        assert!(!first.is_empty());
        assert!(first.len() < node_tables().len());
        for name in &first {
            assert!(node_tables().contains(name));
        }
    }

    #[test]
    fn an_incomplete_cutover_marker_is_refused() {
        let mut store = memory();
        apply(&mut store).unwrap();
        refuse_incomplete_cutover(&mut store).unwrap();
        super::super::bootstrap(&mut store, CUTOVER, CUTOVER_INCOMPLETE).unwrap();
        let refused = refuse_incomplete_cutover(&mut store).unwrap_err();
        assert_eq!(refused.fault, Fault::Schema);
        assert!(refused.message.contains("incomplete"));
    }
}
