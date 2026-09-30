use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[cfg(feature = "mariadb")]
use podmesh::store::{
    migrations::node_tables, DurableStore, MariadbConfig, MariadbStore, Value,
};
#[cfg(feature = "mariadb")]
use rusqlite::types::ValueRef;
#[cfg(feature = "mariadb")]
use sha2::{Digest, Sha256};

const USAGE: &str = "\
Usage: podmesh-storage-migrate --from-sqlite PATH --to-dsn URL [--dry-run] [--force]

Copies an offline SQLite journal into the MariaDB database named by URL,
through the versioned NODE_MIGRATIONS schema. The target must contain no
tables. --force DROPS EVERY TABLE in that database first; take and verify
a backup before using it. The DSN is always redacted from output.";

#[derive(Debug, PartialEq)]
struct Args {
    from_sqlite: PathBuf,
    to_dsn: String,
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
    let mut index = 0;

    while index < values.len() {
        match values[index].as_str() {
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

fn inspect_sqlite(path: &Path) -> Result<Vec<TableCount>, Box<dyn std::error::Error>> {
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

    names
        .into_iter()
        .map(|name| {
            let sql = format!("SELECT COUNT(*) FROM {}", quoted_sqlite_identifier(&name));
            let rows = db.query_row(&sql, [], |row| row.get(0))?;
            Ok(TableCount { name, rows })
        })
        .collect()
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
fn copy_to_mariadb(args: &Args, counts: &[TableCount]) -> Result<(), Box<dyn std::error::Error>> {
    use podmesh::store::migrations;
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

    // The versioned schema, not a heated DDL guess: the same NODE_MIGRATIONS
    // the node opens, in MariaDB dialect.
    let applied = migrations::apply(&mut target)?;
    println!(
        "Schema: {} migrations applied ({} -> {})",
        applied.ran.len(),
        applied.from,
        applied.to
    );

    let ordered = node_tables();
    for name in &ordered {
        let entry = counts
            .iter()
            .find(|count| &count.name == name)
            .map(|count| count.rows)
            .unwrap_or(0);
        let columns = source_columns(&source, name).unwrap_or_default();
        if columns.is_empty() && entry > 0 {
            return Err(format!("SQLite source has no readable columns for {name}").into());
        }
        let rows = if columns.is_empty() {
            Vec::new()
        } else {
            source_rows(&source, name, &columns)?
        };
        if !rows.is_empty() {
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
            let mut transaction = target.transaction()?;
            for row in &rows {
                transaction.execute(&insert, row)?;
            }
            transaction.commit()?;
        }
        println!("Copied {name}: {} rows", rows.len());
    }

    for name in &ordered {
        let columns = source_columns(&source, name).unwrap_or_default();
        let source_rows = if columns.is_empty() {
            Vec::new()
        } else {
            source_rows(&source, name, &columns)?
        };
        let source_checksum = logical_checksum(&source_rows);
        let quoted = quoted_mariadb_identifier(name)?;
        let list = columns
            .iter()
            .map(|column| quoted_mariadb_identifier(column))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let target_rows: Vec<Vec<Value>> = if columns.is_empty() {
            Vec::new()
        } else {
            target
                .query(&format!("SELECT {list} FROM {quoted}"), &[])?
                .into_iter()
                .map(|row| row.into_values())
                .collect()
        };
        let count = target_rows.len() as i64;
        let expected = counts
            .iter()
            .find(|count| &count.name == name)
            .map(|count| count.rows)
            .unwrap_or(0);
        if count != source_rows.len() as i64 || count != expected {
            return Err(format!(
                "row-count verification failed for {name}: SQLite={expected}, copied={count}"
            )
            .into());
        }
        let target_checksum = logical_checksum(&target_rows);
        if source_checksum != target_checksum {
            return Err(format!(
                "logical SHA-256 verification failed for {name} (values or storage classes differ)"
            )
            .into());
        }
        println!("Verified {name}: {count} rows, logical sha256={source_checksum}");
    }
    let integrity = target.integrity_check()?;
    if !integrity.ok {
        return Err(format!(
            "MariaDB integrity check failed: {}",
            integrity.findings.join("; ")
        )
        .into());
    }
    println!(
        "Migration complete: {} tables copied and checked; MariaDB integrity OK",
        ordered.len()
    );
    Ok(())
}

#[cfg(not(feature = "mariadb"))]
fn copy_to_mariadb(_args: &Args, _counts: &[TableCount]) -> Result<(), Box<dyn std::error::Error>> {
    Err("copy mode requires a build with --features mariadb".into())
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.dry_run {
        return run_dry(&args);
    }
    let tables = inspect_sqlite(&args.from_sqlite)?;
    copy_to_mariadb(&args, &tables)
}

fn run_dry(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let counts = inspect_sqlite(&args.from_sqlite)?;
    println!("SQLite source: {}", args.from_sqlite.display());
    println!("MariaDB target: supplied via --to-dsn (value redacted)");
    println!("Tables: {}", counts.len());
    for entry in &counts {
        println!("  {}: {} rows", entry.name, entry.rows);
    }
    println!("Plan:");
    println!("  1. Verify that the MariaDB target has no user tables.");
    println!("  2. Create the versioned MariaDB schema.");
    println!("  3. Copy each table and compare row counts and checksums.");
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
                dry_run: true,
                force: false,
            }
        );
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
            "CREATE TABLE alpha(id INTEGER PRIMARY KEY);
             CREATE TABLE beta(value TEXT NOT NULL);
             INSERT INTO alpha VALUES(1);
             INSERT INTO beta VALUES('one'),('two');",
        )
        .unwrap();
        drop(db);

        let result = inspect_sqlite(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(
            result,
            vec![
                TableCount {
                    name: "alpha".into(),
                    rows: 1
                },
                TableCount {
                    name: "beta".into(),
                    rows: 2
                },
            ]
        );
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
}
