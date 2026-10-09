use podmesh::store::migrations::{
    self, manager_tables, manager_tables_through, manager_version, node_tables, node_tables_through,
    node_version,
};
use podmesh::store::DurableStore;
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[cfg(feature = "mariadb")]
use podmesh::store::{
    bootstrap, schema_version, MariadbConfig, MariadbStore, Value, SCHEMA_TABLE,
};
#[cfg(feature = "mariadb")]
use rusqlite::types::ValueRef;
#[cfg(feature = "mariadb")]
use sha2::{Digest, Sha256};

const USAGE: &str = "\
Usage: podmesh-storage-migrate --from-sqlite PATH --to-dsn URL [--role node|manager] [--dry-run] [--force]

Copies an offline SQLite journal into the MariaDB database named by URL,
through the versioned schema for the role (node: NODE_MIGRATIONS; manager:
MANAGER_MIGRATIONS). The source is validated before any target mutation:
future schema versions, unknown tables, and missing mandatory tables are
refused. The target must contain no tables. --force DROPS EVERY TABLE in that
database first; take and verify a backup before using it. Until copy and
verification finish, store_schema.<role>_cutover = 0 and the store refuses
to open the target. The DSN is always redacted from output.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MigrateRole {
    Node,
    Manager,
}

impl MigrateRole {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "node" => Ok(Self::Node),
            "manager" => Ok(Self::Manager),
            other => Err(format!("--role must be node or manager, not {other}")),
        }
    }

    fn schema_name(self) -> &'static str {
        match self {
            Self::Node => migrations::NODE,
            Self::Manager => migrations::MANAGER,
        }
    }

    fn cutover_marker(self) -> &'static str {
        match self {
            Self::Node => migrations::CUTOVER,
            Self::Manager => migrations::MANAGER_CUTOVER,
        }
    }

    fn version(self) -> i64 {
        match self {
            Self::Node => node_version(),
            Self::Manager => manager_version(),
        }
    }

    fn tables(self) -> Vec<&'static str> {
        match self {
            Self::Node => node_tables(),
            Self::Manager => manager_tables(),
        }
    }

    fn tables_through(self, version: i64) -> Vec<&'static str> {
        match self {
            Self::Node => node_tables_through(version),
            Self::Manager => manager_tables_through(version),
        }
    }

    fn apply(self, store: &mut dyn DurableStore) -> podmesh::store::Result<migrations::Applied> {
        match self {
            Self::Node => migrations::apply(store),
            Self::Manager => migrations::apply_manager(store),
        }
    }

    fn store_label(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Manager => "manager",
        }
    }
}

#[derive(Debug, PartialEq)]
struct Args {
    from_sqlite: PathBuf,
    to_dsn: String,
    role: MigrateRole,
    dry_run: bool,
    force: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct TableCount {
    name: String,
    rows: i64,
}

fn parse_args(values: &[String]) -> Result<Args, String> {
    let mut from_sqlite = None;
    let mut to_dsn = None;
    let mut dry_run = false;
    let mut force = false;
    let mut role = MigrateRole::Node;
    let mut index = 0;

    while index < values.len() {
        match values[index].as_str() {
            "--role" => {
                index += 1;
                let value = values
                    .get(index)
                    .ok_or_else(|| "--role requires node or manager".to_string())?;
                role = MigrateRole::parse(value)?;
            }
            "--from-sqlite" => {
                index += 1;
                let value = values
                    .get(index)
                    .ok_or_else(|| "--from-sqlite requires a path".to_string())?;
                if from_sqlite.replace(PathBuf::from(value)).is_some() {
                    return Err("--from-sqlite may be supplied only once".into());
                }
            }
            "--to-dsn" => {
                index += 1;
                let value = values
                    .get(index)
                    .ok_or_else(|| "--to-dsn requires a URL".to_string())?;
                if to_dsn.replace(value.clone()).is_some() {
                    return Err("--to-dsn may be supplied only once".into());
                }
            }
            "--dry-run" => {
                if dry_run {
                    return Err("--dry-run may be supplied only once".into());
                }
                dry_run = true;
            }
            "--force" => {
                if force {
                    return Err("--force may be supplied only once".into());
                }
                force = true;
            }
            value => return Err(format!("unknown argument: {value}")),
        }
        index += 1;
    }

    if dry_run && force {
        return Err("--force cannot be combined with --dry-run".into());
    }
    Ok(Args {
        from_sqlite: from_sqlite.ok_or_else(|| "--from-sqlite is required".to_string())?,
        to_dsn: to_dsn.ok_or_else(|| "--to-dsn is required".to_string())?,
        role,
        dry_run,
        force,
    })
}

fn quoted_sqlite_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[allow(dead_code)]
fn quoted_identifier(name: &str) -> String {
    quoted_sqlite_identifier(name)
}

#[derive(Clone, Debug, PartialEq)]
struct SourceJournal {
    counts: Vec<TableCount>,
    /// Recorded `store_schema` version for this role, when present.
    version: Option<i64>,
}

fn inspect_sqlite(path: &Path, role: MigrateRole) -> Result<SourceJournal, Box<dyn std::error::Error>> {
    let db = open_source(path)?;
    let mut statement = db.prepare(
        "SELECT name
         FROM sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let counts = names
        .into_iter()
        .map(|name| {
            let sql = format!("SELECT COUNT(*) FROM {}", quoted_sqlite_identifier(&name));
            let rows = db.query_row(&sql, [], |row| row.get(0))?;
            Ok(TableCount { name, rows })
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

    let version = if counts.iter().any(|table| table.name == "store_schema") {
        match db.query_row(
            "SELECT version FROM store_schema WHERE name = ?1",
            [role.schema_name()],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(version) => Some(version),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(error.into()),
        }
    } else if role == MigrateRole::Manager {
        let legacy: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match legacy {
            0 => None,
            3 => Some(manager_version()),
            other => {
                return Err(format!(
                    "SQLite source carries PRAGMA user_version = {other}; this build knows manager schema version {latest} (legacy user_version 3)",
                    latest = manager_version()
                )
                .into());
            }
        }
    } else {
        None
    };

    Ok(SourceJournal { counts, version })
}

/// Validate the SQLite source before any MariaDB mutation.
///
/// - Empty / foreign files are refused.
/// - Future `node` schema versions are refused.
/// - Tables outside the node inventory (except `store_schema`) are refused.
/// - At the current version, every inventory table must exist.
/// - At an older recorded version *N*, only tables from migrations 1..=N are
///   mandatory; later inventory tables may be absent (copied as empty).
/// - A pre-schema journal may omit inventory tables, but must carry at least
///   one recognized node table.
fn validate_source(journal: &SourceJournal, role: MigrateRole) -> Result<(), Box<dyn std::error::Error>> {
    let present: BTreeSet<&str> = journal
        .counts
        .iter()
        .map(|table| table.name.as_str())
        .collect();
    let inventory: BTreeSet<&str> = role.tables().into_iter().collect();
    let allowed: BTreeSet<&str> = inventory
        .iter()
        .copied()
        .chain(std::iter::once("store_schema"))
        .collect();

    if present.is_empty() {
        return Err("SQLite source carries no user tables; refusing an empty or foreign file".into());
    }

    let unknown: Vec<&str> = present
        .iter()
        .copied()
        .filter(|name| !allowed.contains(name))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "SQLite source has unknown tables not in the {} inventory: {}",
            role.store_label(),
            unknown.join(", ")
        )
        .into());
    }

    let latest = role.version();
    let label = role.store_label();
    match journal.version {
        Some(version) if version > latest => {
            return Err(format!(
                "SQLite source carries {label} schema version {version} and this build knows {latest}: \
                 a journal is never opened by a build older than the one that wrote it"
            )
            .into());
        }
        Some(version) if version == latest => {
            let missing: Vec<&str> = inventory
                .iter()
                .copied()
                .filter(|name| !present.contains(name))
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "SQLite source at {label} schema version {version} is missing mandatory tables: {}",
                    missing.join(", ")
                )
                .into());
            }
        }
        Some(version) => {
            let mandatory = role.tables_through(version);
            let missing: Vec<&str> = mandatory
                .into_iter()
                .filter(|name| !present.contains(name))
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "SQLite source at {label} schema version {version} is missing tables that version requires: {}",
                    missing.join(", ")
                )
                .into());
            }
        }
        None => {
            let recognized = present.iter().any(|name| inventory.contains(name));
            if !recognized {
                return Err(format!(
                    "SQLite source has no store_schema.{label} version and no recognized {label} tables; refusing"
                )
                .into());
            }
        }
    }
    Ok(())
}

fn open_source(path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

fn quoted_mariadb_identifier(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    if name.is_empty() || name.as_bytes().len() > 64 || name.contains('\0') {
        return Err(format!("unusable MariaDB identifier: {name:?}").into());
    }
    Ok(format!("`{}`", name.replace('`', "``")))
}

#[cfg(feature = "mariadb")]
fn sqlite_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Integer(value),
        ValueRef::Real(value) => Value::Real(value),
        ValueRef::Text(value) => match std::str::from_utf8(value) {
            Ok(value) => Value::Text(value.to_string()),
            Err(_) => Value::Blob(value.to_vec()),
        },
        ValueRef::Blob(value) => Value::Blob(value.to_vec()),
    }
}

#[cfg(feature = "mariadb")]
fn source_rows(
    db: &Connection,
    table: &str,
    columns: &[String],
) -> Result<Vec<Vec<Value>>, Box<dyn std::error::Error>> {
    let list = columns
        .iter()
        .map(|column| quoted_sqlite_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = db.prepare(&format!(
        "SELECT {list} FROM {}",
        quoted_sqlite_identifier(table)
    ))?;
    let mut rows = statement.query([])?;
    let mut collected = Vec::new();
    while let Some(row) = rows.next()? {
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(sqlite_value(row.get_ref(index)?));
        }
        collected.push(values);
    }
    Ok(collected)
}

#[cfg(feature = "mariadb")]
fn source_columns(db: &Connection, table: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let sql = format!(
        "SELECT name FROM pragma_table_info({}) ORDER BY cid",
        quoted_sqlite_literal(table)
    );
    let mut statement = db.prepare(&sql)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

#[cfg(feature = "mariadb")]
fn quoted_sqlite_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(feature = "mariadb")]
fn update_value_digest(digest: &mut Sha256, value: &Value) {
    match value {
        Value::Null => digest.update([0]),
        Value::Integer(value) => {
            digest.update([1]);
            digest.update(value.to_be_bytes());
        }
        Value::Real(value) => {
            digest.update([2]);
            digest.update(value.to_bits().to_be_bytes());
        }
        Value::Text(value) => {
            digest.update([3]);
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Value::Blob(value) => {
            digest.update([4]);
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
    }
}

#[cfg(feature = "mariadb")]
fn logical_checksum(rows: &[Vec<Value>]) -> String {
    let mut row_digests = rows
        .iter()
        .map(|row| {
            let mut digest = Sha256::new();
            digest.update((row.len() as u64).to_be_bytes());
            for value in row {
                update_value_digest(&mut digest, value);
            }
            digest.finalize().to_vec()
        })
        .collect::<Vec<_>>();
    row_digests.sort();
    let mut digest = Sha256::new();
    digest.update((row_digests.len() as u64).to_be_bytes());
    for row in row_digests {
        digest.update(row);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "mariadb")]
fn clear_target(store: &mut MariadbStore) -> Result<(), Box<dyn std::error::Error>> {
    let tables = store.tables()?;
    store.execute_batch("SET FOREIGN_KEY_CHECKS = 0")?;
    let dropped = tables.iter().try_for_each(|table| {
        store.execute_batch(&format!(
            "DROP TABLE {}",
            quoted_mariadb_identifier(table)
                .map_err(|error| podmesh::store::Fault::Schema.error(error.to_string()))?
        ))
    });
    let restored = store.execute_batch("SET FOREIGN_KEY_CHECKS = 1");
    dropped?;
    restored?;
    Ok(())
}

#[cfg(feature = "mariadb")]
fn mark_cutover_incomplete(
    store: &mut MariadbStore,
    role: MigrateRole,
) -> Result<(), Box<dyn std::error::Error>> {
    bootstrap(store, role.cutover_marker(), migrations::CUTOVER_INCOMPLETE)?;
    Ok(())
}

#[cfg(feature = "mariadb")]
fn clear_cutover_marker(
    store: &mut MariadbStore,
    role: MigrateRole,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = store.transaction()?;
    tx.execute(
        "DELETE FROM store_schema WHERE name = ?",
        &[Value::from(role.cutover_marker())],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(feature = "mariadb")]
fn copy_to_mariadb(args: &Args, journal: &SourceJournal) -> Result<(), Box<dyn std::error::Error>> {
    validate_source(journal, args.role)?;
    let counts = &journal.counts;
    let source = open_source(&args.from_sqlite)?;

    let config = MariadbConfig::from_dsn(&args.to_dsn);
    let mut target = MariadbStore::open(&config)?;
    if target.database()?.is_none() {
        return Err("the target DSN selects no MariaDB database".into());
    }
    println!("MariaDB target: {}", target.target());
    println!("Durability gate: global innodb_flush_log_at_trx_commit = 1");

    let existing = target.tables()?;
    if !existing.is_empty() && !args.force {
        return Err(format!(
            "refusing non-empty MariaDB target ({} tables); take a verified backup, then use --force to DROP EVERY TARGET TABLE",
            existing.len()
        )
        .into());
    }
    if args.force {
        println!("DANGER: --force accepted; dropping every table in the selected MariaDB database");
        clear_target(&mut target)?;
    }

    // Incomplete before schema or data: a crash leaves the store refusing to open.
    mark_cutover_incomplete(&mut target, args.role)?;
    println!(
        "Cutover lock: {} = {}",
        args.role.cutover_marker(),
        migrations::CUTOVER_INCOMPLETE
    );

    let applied = args.role.apply(&mut target)?;
    println!(
        "Schema: {} migrations applied ({} -> {})",
        applied.ran.len(),
        applied.from,
        applied.to
    );
    // Re-assert after apply: migrations rewrite store_schema rows one by one.
    mark_cutover_incomplete(&mut target, args.role)?;

    let ordered = args.role.tables();
    let mut prepared = Vec::new();
    for name in &ordered {
        let present = counts.iter().any(|count| &count.name == *name);
        let columns = if present {
            source_columns(&source, name)?
        } else {
            Vec::new()
        };
        let expected = counts
            .iter()
            .find(|count| &count.name == *name)
            .map(|count| count.rows)
            .unwrap_or(0);
        if columns.is_empty() && expected > 0 {
            return Err(format!("SQLite source has no readable columns for {name}").into());
        }
        let rows = if columns.is_empty() {
            Vec::new()
        } else {
            source_rows(&source, name, &columns)?
        };
        if rows.len() as i64 != expected {
            return Err(format!(
                "SQLite source row count for {name} changed during read: expected {expected}, got {}",
                rows.len()
            )
            .into());
        }
        prepared.push((*name, columns, rows, expected));
    }

    // One transaction for every row of every table: commit all or leave nothing.
    {
        let mut transaction = target.transaction()?;
        for (name, columns, rows, _) in &prepared {
            if rows.is_empty() {
                continue;
            }
            let names = columns
                .iter()
                .map(|column| quoted_mariadb_identifier(column))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            let placeholders = vec!["?"; columns.len()].join(", ");
            let insert = format!(
                "INSERT INTO {} ({names}) VALUES ({placeholders})",
                quoted_mariadb_identifier(name)?
            );
            for row in rows {
                transaction.execute(&insert, row)?;
            }
        }
        transaction.commit()?;
    }
    for (name, _, rows, _) in &prepared {
        println!("Copied {name}: {} rows", rows.len());
    }

    for (name, columns, source_rows, expected) in &prepared {
        let source_checksum = logical_checksum(source_rows);
        let quoted = quoted_mariadb_identifier(name)?;
        let target_rows: Vec<Vec<Value>> = if columns.is_empty() {
            Vec::new()
        } else {
            let list = columns
                .iter()
                .map(|column| quoted_mariadb_identifier(column))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            target
                .query(&format!("SELECT {list} FROM {quoted}"), &[])?
                .into_iter()
                .map(|row| row.into_values())
                .collect()
        };
        let count = target_rows.len() as i64;
        if count != source_rows.len() as i64 || count != *expected {
            return Err(format!(
                "row-count verification failed for {name}: SQLite={expected}, copied={count}; \
                 cutover lock remains; the {role} store will refuse this target",
                role = args.role.store_label()
            )
            .into());
        }
        let target_checksum = logical_checksum(&target_rows);
        if source_checksum != target_checksum {
            return Err(format!(
                "logical SHA-256 verification failed for {name} (values or storage classes differ); \
                 cutover lock remains; the {role} store will refuse this target",
                role = args.role.store_label()
            )
            .into());
        }
        println!("Verified {name}: {count} rows, logical sha256={source_checksum}");
    }

    let integrity = target.integrity_check()?;
    if !integrity.ok {
        return Err(format!(
            "MariaDB integrity check failed: {}; cutover lock remains",
            integrity.findings.join("; ")
        )
        .into());
    }

    clear_cutover_marker(&mut target, args.role)?;
    if schema_version(&mut target, args.role.cutover_marker())?.is_some() {
        return Err(format!(
            "failed to clear store_schema.{} after verification",
            args.role.cutover_marker()
        )
        .into());
    }
    // store_schema itself is not in node_tables(); ensure it remains.
    if !target
        .tables()?
        .iter()
        .any(|table| table == SCHEMA_TABLE)
    {
        return Err("store_schema missing after cutover".into());
    }

    println!(
        "Migration complete: {} tables copied and checked; cutover lock cleared; MariaDB integrity OK",
        ordered.len()
    );
    Ok(())
}

#[cfg(not(feature = "mariadb"))]
fn copy_to_mariadb(_args: &Args, _journal: &SourceJournal) -> Result<(), Box<dyn std::error::Error>> {
    Err("copy mode requires a build with --features mariadb".into())
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.dry_run {
        return run_dry(&args);
    }
    let journal = inspect_sqlite(&args.from_sqlite, args.role)?;
    validate_source(&journal, args.role)?;
    copy_to_mariadb(&args, &journal)
}

fn run_dry(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let journal = inspect_sqlite(&args.from_sqlite, args.role)?;
    validate_source(&journal, args.role)?;
    println!("Role: {}", args.role.store_label());
    println!("SQLite source: {}", args.from_sqlite.display());
    println!("MariaDB target: supplied via --to-dsn (value redacted)");
    match journal.version {
        Some(version) => println!("Source {} schema version: {version}", args.role.store_label()),
        None => println!(
            "Source {} schema version: none (pre-schema journal)",
            args.role.store_label()
        ),
    }
    println!("Tables: {}", journal.counts.len());
    for entry in &journal.counts {
        println!("  {}: {} rows", entry.name, entry.rows);
    }
    println!("Plan:");
    println!("  1. Source already validated (version, unknown tables, mandatory tables).");
    println!("  2. Verify that the MariaDB target has no user tables.");
    println!("  3. Set cutover lock, create the versioned MariaDB schema.");
    println!("  4. Copy every table in one transaction; verify counts and checksums.");
    println!("  5. Clear cutover lock only after verification.");
    println!("Dry run only: MariaDB was not contacted and no data was changed.");
    Ok(())
}

fn main() {
    let values = std::env::args().skip(1).collect::<Vec<_>>();
    if values
        .iter()
        .any(|value| value == "--help" || value == "-h")
    {
        println!("{USAGE}");
        return;
    }

    let result = parse_args(&values)
        .map_err(|error| format!("{error}\n{USAGE}"))
        .and_then(|args| run(args).map_err(|error| error.to_string()));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_required_arguments_and_dry_run() {
        let values = [
            "--from-sqlite",
            "/var/lib/podmesh/state.sqlite",
            "--to-dsn",
            "mysql://example.invalid/podmesh-node",
            "--dry-run",
        ]
        .map(String::from);

        assert_eq!(
            parse_args(&values).unwrap(),
            Args {
                from_sqlite: PathBuf::from("/var/lib/podmesh/state.sqlite"),
                to_dsn: "mysql://example.invalid/podmesh-node".into(),
                role: MigrateRole::Node,
                dry_run: true,
                force: false,
            }
        );
    }

    #[test]
    fn parses_manager_role() {
        let values = [
            "--role",
            "manager",
            "--from-sqlite",
            "manager.sqlite",
            "--to-dsn",
            "mysql://example.invalid/podmesh-manager",
            "--dry-run",
        ]
        .map(String::from);
        assert_eq!(parse_args(&values).unwrap().role, MigrateRole::Manager);
    }

    #[test]
    fn rejects_missing_target() {
        let values = ["--from-sqlite", "state.sqlite"].map(String::from);
        assert_eq!(parse_args(&values).unwrap_err(), "--to-dsn is required");
    }

    #[test]
    fn quotes_sqlite_identifiers() {
        assert_eq!(quoted_sqlite_identifier("odd\"table"), "\"odd\"\"table\"");
    }

    #[test]
    fn inspects_table_names_and_row_counts_read_only() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "podmesh-storage-migrate-{}-{nonce}.sqlite",
            std::process::id()
        ));
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE observations(id INTEGER PRIMARY KEY, observed_at INTEGER NOT NULL, operation TEXT NOT NULL, result TEXT NOT NULL);
             INSERT INTO metadata VALUES('host_uuid', 'x');
             INSERT INTO observations(observed_at, operation, result) VALUES(1, 'op', 'ok'), (2, 'op', 'ok');",
        )
        .unwrap();
        drop(db);

        let result = inspect_sqlite(&path, MigrateRole::Node).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(result.version, None);
        assert_eq!(
            result.counts,
            vec![
                TableCount {
                    name: "metadata".into(),
                    rows: 1
                },
                TableCount {
                    name: "observations".into(),
                    rows: 2
                },
            ]
        );
        validate_source(&result, MigrateRole::Node).unwrap();
    }

    #[test]
    fn rejects_force_with_dry_run() {
        let values = [
            "--from-sqlite",
            "state.sqlite",
            "--to-dsn",
            "mysql://example.invalid/podmesh-node",
            "--dry-run",
            "--force",
        ]
        .map(String::from);
        assert_eq!(
            parse_args(&values).unwrap_err(),
            "--force cannot be combined with --dry-run"
        );
    }

    #[test]
    fn refuses_an_empty_sqlite_file() {
        let path = std::env::temp_dir().join(format!(
            "podmesh-migrate-empty-{}-{}.sqlite",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Connection::open(&path).unwrap();
        let journal = inspect_sqlite(&path, MigrateRole::Node).unwrap();
        std::fs::remove_file(&path).unwrap();
        let refused = validate_source(&journal, MigrateRole::Node).unwrap_err().to_string();
        assert!(refused.contains("no user tables"), "{refused}");
    }

    #[test]
    fn refuses_unknown_tables_and_future_versions() {
        let unknown = SourceJournal {
            counts: vec![TableCount {
                name: "not_a_node_table".into(),
                rows: 1,
            }],
            version: None,
        };
        let refused = validate_source(&unknown, MigrateRole::Node).unwrap_err().to_string();
        assert!(refused.contains("unknown tables"), "{refused}");

        let future = SourceJournal {
            counts: node_tables()
                .into_iter()
                .chain(std::iter::once("store_schema"))
                .map(|name| TableCount {
                    name: name.into(),
                    rows: 0,
                })
                .collect(),
            version: Some(node_version() + 1),
        };
        let refused = validate_source(&future, MigrateRole::Node).unwrap_err().to_string();
        assert!(refused.contains("older than the one that wrote it"), "{refused}");
    }

    #[test]
    fn refuses_missing_mandatory_tables_at_current_version() {
        let journal = SourceJournal {
            counts: vec![
                TableCount {
                    name: "store_schema".into(),
                    rows: 1,
                },
                TableCount {
                    name: "metadata".into(),
                    rows: 0,
                },
            ],
            version: Some(node_version()),
        };
        let refused = validate_source(&journal, MigrateRole::Node).unwrap_err().to_string();
        assert!(refused.contains("missing mandatory tables"), "{refused}");
    }

    #[test]
    fn manager_legacy_user_version_three_maps_to_current_schema() {
        let path = std::env::temp_dir().join(format!(
            "podmesh-migrate-manager-legacy-{}-{}.sqlite",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "PRAGMA user_version = 3;
             CREATE TABLE identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), replica_id TEXT NOT NULL, topology_json TEXT NOT NULL);
             CREATE TABLE facts (event_id TEXT PRIMARY KEY, fact_json TEXT NOT NULL, sha256 TEXT NOT NULL);
             CREATE TABLE receipts (operation_id TEXT PRIMARY KEY, kind TEXT NOT NULL, source_replica_id TEXT, wire_operation_id TEXT, request_json TEXT NOT NULL, response_json TEXT NOT NULL, sha256 TEXT NOT NULL);
             CREATE TABLE exchange_audit_events (audit_event_id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL, wire_nonce TEXT NOT NULL, direction TEXT NOT NULL, phase TEXT NOT NULL, authenticated_peer_id TEXT, peer_claim TEXT, operation_id TEXT, request_frame_bytes INTEGER NOT NULL, request_announced_body_bytes INTEGER, request_sha256 TEXT, reply_frame_bytes INTEGER NOT NULL, reply_announced_body_bytes INTEGER, reply_sha256 TEXT, outcome TEXT NOT NULL, error_category TEXT, reason_code TEXT, local_receipt_operation_id TEXT, local_receipt_sha256 TEXT, remote_receipt_operation_id TEXT, remote_receipt_sha256 TEXT, replayed INTEGER NOT NULL CHECK(replayed IN (0, 1)), record_json TEXT NOT NULL, sha256 TEXT NOT NULL, UNIQUE(direction, attempt_id, phase));",
        )
        .unwrap();
        drop(db);
        let journal = inspect_sqlite(&path, MigrateRole::Manager).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(journal.version, Some(manager_version()));
        validate_source(&journal, MigrateRole::Manager).unwrap();
    }
}
