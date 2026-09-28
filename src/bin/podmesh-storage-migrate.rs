use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

#[cfg(feature = "mariadb")]
use podmesh::store::{DurableStore, MariadbConfig, MariadbStore, Value};
#[cfg(feature = "mariadb")]
use rusqlite::types::ValueRef;
#[cfg(feature = "mariadb")]
use sha2::{Digest, Sha256};

const USAGE: &str = "\
Usage: podmesh-storage-migrate --from-sqlite PATH --to-dsn URL [--dry-run] [--force]

Copies an offline SQLite journal into the MariaDB database named by URL.
The target must contain no tables. --force DROPS EVERY TABLE in that database first;
take and verify a backup before using it. The DSN is always redacted from output.";

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

#[cfg(feature = "mariadb")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Affinity {
    Integer,
    Text,
    Blob,
    Real,
    Numeric,
}

#[cfg(feature = "mariadb")]
#[derive(Clone, Debug)]
struct Column {
    name: String,
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

#[cfg(feature = "mariadb")]
#[derive(Clone, Debug)]
struct Index {
    name: String,
    unique: bool,
    columns: Vec<String>,
}

#[cfg(feature = "mariadb")]
#[derive(Clone, Debug)]
struct Table {
    name: String,
    rows: i64,
    columns: Vec<Column>,
    indexes: Vec<Index>,
}

#[cfg(feature = "mariadb")]
fn sqlite_affinity(declared_type: &str) -> Affinity {
    let kind = declared_type.to_ascii_uppercase();
    if kind.contains("INT") {
        Affinity::Integer
    } else if kind.contains("CHAR") || kind.contains("CLOB") || kind.contains("TEXT") {
        Affinity::Text
    } else if kind.contains("BLOB") || kind.is_empty() {
        Affinity::Blob
    } else if kind.contains("REAL") || kind.contains("FLOA") || kind.contains("DOUB") {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

#[cfg(feature = "mariadb")]
fn quoted_sqlite_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(feature = "mariadb")]
fn quoted_mariadb_identifier(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    if name.is_empty() || name.as_bytes().len() > 64 || name.contains('\0') {
        return Err(format!(
            "SQLite identifier {name:?} cannot be represented by MariaDB (1..64 bytes, no NUL)"
        )
        .into());
    }
    Ok(format!("`{}`", name.replace('`', "``")))
}

#[cfg(feature = "mariadb")]
fn load_schema(
    db: &Connection,
    counts: &[TableCount],
) -> Result<Vec<Table>, Box<dyn std::error::Error>> {
    let mut tables = Vec::with_capacity(counts.len());
    for count in counts {
        quoted_mariadb_identifier(&count.name)?;
        let source_sql: String = db.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?",
            [&count.name],
            |row| row.get(0),
        )?;
        if source_sql
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("CREATE VIRTUAL TABLE")
        {
            return Err(format!(
                "{} is a virtual table; migrate it through an engine-specific schema",
                count.name
            )
            .into());
        }

        let pragma = format!("PRAGMA table_xinfo({})", quoted_sqlite_literal(&count.name));
        let mut statement = db.prepare(&pragma)?;
        let columns = statement
            .query_map([], |row| {
                let hidden: i64 = row.get(6)?;
                Ok((
                    Column {
                        name: row.get(1)?,
                        declared_type: row.get(2)?,
                        not_null: row.get::<_, i64>(3)? != 0,
                        default_value: row.get(4)?,
                        primary_key_position: row.get(5)?,
                    },
                    hidden,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some((column, hidden)) = columns.iter().find(|(_, hidden)| *hidden != 0) {
            return Err(format!(
                "{}.{} is a generated or hidden SQLite column (hidden={hidden}); use an engine-specific schema",
                count.name, column.name
            )
            .into());
        }
        let columns = columns
            .into_iter()
            .map(|(column, _)| {
                quoted_mariadb_identifier(&column.name)?;
                Ok(column)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        if columns.is_empty() {
            return Err(format!("{} has no copyable columns", count.name).into());
        }

        let foreign_key_pragma = format!(
            "PRAGMA foreign_key_list({})",
            quoted_sqlite_literal(&count.name)
        );
        let mut foreign_key_statement = db.prepare(&foreign_key_pragma)?;
        let mut foreign_keys = foreign_key_statement.query([])?;
        if foreign_keys.next()?.is_some() {
            return Err(format!(
                "{} has foreign keys; use the versioned MariaDB schema so referential actions are preserved",
                count.name
            )
            .into());
        }

        let index_pragma = format!("PRAGMA index_list({})", quoted_sqlite_literal(&count.name));
        let mut statement = db.prepare(&index_pragma)?;
        let listed = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? != 0,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut indexes = Vec::new();
        for (position, (name, unique, origin, partial)) in listed.into_iter().enumerate() {
            if origin == "pk" {
                continue;
            }
            if partial {
                return Err(format!(
                    "{} index {} is partial; use the versioned MariaDB schema so its predicate is preserved",
                    count.name, name
                )
                .into());
            }
            let columns_pragma = format!("PRAGMA index_xinfo({})", quoted_sqlite_literal(&name));
            let mut statement = db.prepare(&columns_pragma)?;
            let parts = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(5)? != 0,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut index_columns = Vec::new();
            for (cid, column, key) in parts {
                if !key {
                    continue;
                }
                if cid < 0 || column.is_none() {
                    return Err(format!(
                        "{} index {} contains an expression; use an engine-specific schema",
                        count.name, name
                    )
                    .into());
                }
                index_columns.push(column.expect("checked above"));
            }
            if index_columns.is_empty() {
                return Err(format!("{} index {} has no columns", count.name, name).into());
            }
            let target_name = if name.starts_with("sqlite_autoindex_") {
                format!("pm_{}_{}", if unique { "u" } else { "i" }, position)
            } else {
                name
            };
            quoted_mariadb_identifier(&target_name)?;
            indexes.push(Index {
                name: target_name,
                unique,
                columns: index_columns,
            });
        }
        tables.push(Table {
            name: count.name.clone(),
            rows: count.rows,
            columns,
            indexes,
        });
    }
    Ok(tables)
}

#[cfg(feature = "mariadb")]
fn strip_outer_parentheses(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim();
        if !(trimmed.starts_with('(') && trimmed.ends_with(')')) {
            return trimmed;
        }
        let mut depth = 0_i64;
        let mut quote = None;
        let mut encloses_all = true;
        for (offset, character) in trimmed.char_indices() {
            match quote {
                Some(open) if character == open => quote = None,
                Some(_) => {}
                None if matches!(character, '\'' | '"') => quote = Some(character),
                None if character == '(' => depth += 1,
                None if character == ')' => {
                    depth -= 1;
                    if depth == 0 && offset != trimmed.len() - 1 {
                        encloses_all = false;
                        break;
                    }
                }
                None => {}
            }
        }
        if !encloses_all || depth != 0 {
            return trimmed;
        }
        value = &trimmed[1..trimmed.len() - 1];
    }
}

#[cfg(feature = "mariadb")]
fn mariadb_default(value: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = strip_outer_parentheses(value);
    let upper = value.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "NULL" | "CURRENT_TIME" | "CURRENT_DATE" | "CURRENT_TIMESTAMP"
    ) || value.parse::<i64>().is_ok()
        || value.parse::<f64>().is_ok()
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        return Ok(value.to_string());
    }
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        let held = value[1..value.len() - 1].replace("\"\"", "\"");
        return Ok(format!("'{}'", held.replace('\'', "''")));
    }
    Err(format!("SQLite default {value:?} has no safe automatic MariaDB translation").into())
}

#[cfg(feature = "mariadb")]
fn indexed_text_limits(table: &Table) -> std::collections::HashMap<String, usize> {
    let mut limits: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let primary_key = table
        .columns
        .iter()
        .filter(|column| column.primary_key_position > 0)
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    for key in std::iter::once(&primary_key).chain(table.indexes.iter().map(|index| &index.columns))
    {
        let text_columns = key
            .iter()
            .filter(|name| {
                table
                    .columns
                    .iter()
                    .find(|column| column.name == **name)
                    .is_some_and(|column| sqlite_affinity(&column.declared_type) == Affinity::Text)
            })
            .count();
        if text_columns == 0 {
            continue;
        }
        let limit = 760 / text_columns;
        for name in key {
            if table
                .columns
                .iter()
                .find(|column| column.name == *name)
                .is_some_and(|column| sqlite_affinity(&column.declared_type) == Affinity::Text)
            {
                limits
                    .entry(name.clone())
                    .and_modify(|held| *held = (*held).min(limit))
                    .or_insert(limit);
            }
        }
    }
    limits
}

#[cfg(feature = "mariadb")]
fn validate_indexed_text_lengths(
    db: &Connection,
    table: &Table,
) -> Result<(), Box<dyn std::error::Error>> {
    for (name, limit) in indexed_text_limits(table) {
        let sql = format!(
            "SELECT COALESCE(MAX(LENGTH({})), 0) FROM {}",
            quoted_sqlite_identifier(&name),
            quoted_sqlite_identifier(&table.name)
        );
        let longest: i64 = db.query_row(&sql, [], |row| row.get(0))?;
        if longest > limit as i64 {
            return Err(format!(
                "{}.{} contains {longest} characters, beyond the safe MariaDB index limit of {limit}; use the versioned schema",
                table.name, name
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(feature = "mariadb")]
fn create_table_sql(table: &Table) -> Result<String, Box<dyn std::error::Error>> {
    let limits = indexed_text_limits(table);
    let primary_key = table
        .columns
        .iter()
        .filter(|column| column.primary_key_position > 0)
        .collect::<Vec<_>>();
    let single_integer_primary = primary_key.len() == 1
        && sqlite_affinity(&primary_key[0].declared_type) == Affinity::Integer;
    let mut definitions = Vec::with_capacity(table.columns.len() + 1);
    for column in &table.columns {
        let kind = match sqlite_affinity(&column.declared_type) {
            Affinity::Integer => "BIGINT".to_string(),
            Affinity::Text => limits.get(&column.name).map_or_else(
                || "LONGTEXT".to_string(),
                |limit| format!("VARCHAR({limit})"),
            ),
            Affinity::Blob => "LONGBLOB".to_string(),
            Affinity::Real => "DOUBLE".to_string(),
            Affinity::Numeric => "DECIMAL(65,30)".to_string(),
        };
        let mut definition = format!("{} {kind}", quoted_mariadb_identifier(&column.name)?);
        if column.not_null || column.primary_key_position > 0 {
            definition.push_str(" NOT NULL");
        } else {
            definition.push_str(" NULL");
        }
        if single_integer_primary && column.primary_key_position > 0 {
            definition.push_str(" AUTO_INCREMENT");
        }
        if let Some(default) = &column.default_value {
            definition.push_str(" DEFAULT ");
            definition.push_str(&mariadb_default(default)?);
        }
        definitions.push(definition);
    }
    if !primary_key.is_empty() {
        let names = primary_key
            .iter()
            .map(|column| {
                Ok((
                    column.primary_key_position,
                    quoted_mariadb_identifier(&column.name)?,
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let mut names = names;
        names.sort_by_key(|(position, _)| *position);
        definitions.push(format!(
            "PRIMARY KEY ({})",
            names
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(format!(
        "CREATE TABLE {} (\n  {}\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        quoted_mariadb_identifier(&table.name)?,
        definitions.join(",\n  ")
    ))
}

#[cfg(feature = "mariadb")]
fn create_index_sql(table: &Table, index: &Index) -> Result<String, Box<dyn std::error::Error>> {
    let columns = index
        .columns
        .iter()
        .map(|column| quoted_mariadb_identifier(column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    Ok(format!(
        "CREATE {}INDEX {} ON {} ({columns})",
        if index.unique { "UNIQUE " } else { "" },
        quoted_mariadb_identifier(&index.name)?,
        quoted_mariadb_identifier(&table.name)?
    ))
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
    table: &Table,
) -> Result<Vec<Vec<Value>>, Box<dyn std::error::Error>> {
    let columns = table
        .columns
        .iter()
        .map(|column| quoted_sqlite_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = db.prepare(&format!(
        "SELECT {columns} FROM {}",
        quoted_sqlite_identifier(&table.name)
    ))?;
    let mut rows = statement.query([])?;
    let mut collected = Vec::new();
    while let Some(row) = rows.next()? {
        let mut values = Vec::with_capacity(table.columns.len());
        for index in 0..table.columns.len() {
            values.push(sqlite_value(row.get_ref(index)?));
        }
        collected.push(values);
    }
    Ok(collected)
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
fn target_rows(
    store: &mut MariadbStore,
    table: &Table,
) -> Result<Vec<Vec<Value>>, Box<dyn std::error::Error>> {
    let columns = table
        .columns
        .iter()
        .map(|column| quoted_mariadb_identifier(&column.name))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let rows = store.query(
        &format!(
            "SELECT {columns} FROM {}",
            quoted_mariadb_identifier(&table.name)?
        ),
        &[],
    )?;
    Ok(rows.into_iter().map(|row| row.into_values()).collect())
}

#[cfg(feature = "mariadb")]
fn clear_target(store: &mut MariadbStore) -> Result<(), Box<dyn std::error::Error>> {
    let tables = store.tables()?;
    store.execute_batch("SET FOREIGN_KEY_CHECKS = 0")?;
    let dropped = tables.iter().try_for_each(|table| {
        store.execute_batch(&format!(
            "DROP TABLE {}",
            quoted_mariadb_identifier(table)
                .map_err(|error| { podmesh::store::Fault::Schema.error(error.to_string()) })?
        ))
    });
    let restored = store.execute_batch("SET FOREIGN_KEY_CHECKS = 1");
    dropped?;
    restored?;
    Ok(())
}

#[cfg(feature = "mariadb")]
fn copy_to_mariadb(args: &Args, counts: &[TableCount]) -> Result<(), Box<dyn std::error::Error>> {
    let source = open_source(&args.from_sqlite)?;
    let tables = load_schema(&source, counts)?;
    for table in &tables {
        validate_indexed_text_lengths(&source, table)?;
        create_table_sql(table)?;
        for index in &table.indexes {
            create_index_sql(table, index)?;
        }
    }

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

    for table in &tables {
        target.execute_batch(&create_table_sql(table)?)?;
    }
    for table in &tables {
        let rows = source_rows(&source, table)?;
        let names = table
            .columns
            .iter()
            .map(|column| quoted_mariadb_identifier(&column.name))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let placeholders = vec!["?"; table.columns.len()].join(", ");
        let insert = format!(
            "INSERT INTO {} ({names}) VALUES ({placeholders})",
            quoted_mariadb_identifier(&table.name)?
        );
        let mut transaction = target.transaction()?;
        for row in &rows {
            transaction.execute(&insert, row)?;
        }
        transaction.commit()?;
        println!("Copied {}: {} rows", table.name, rows.len());
    }
    for table in &tables {
        for index in &table.indexes {
            target.execute_batch(&create_index_sql(table, index)?)?;
        }
    }

    for table in &tables {
        let count = target
            .query_one(
                &format!(
                    "SELECT COUNT(*) FROM {}",
                    quoted_mariadb_identifier(&table.name)?
                ),
                &[],
            )?
            .ok_or_else(|| format!("MariaDB returned no count for {}", table.name))?
            .integer(0)?;
        if count != table.rows {
            return Err(format!(
                "row-count verification failed for {}: SQLite={}, MariaDB={count}",
                table.name, table.rows
            )
            .into());
        }
        let source_checksum = logical_checksum(&source_rows(&source, table)?);
        let target_checksum = logical_checksum(&target_rows(&mut target, table)?);
        if source_checksum != target_checksum {
            return Err(format!(
                "logical SHA-256 verification failed for {} (values or storage classes differ)",
                table.name
            )
            .into());
        }
        println!(
            "Verified {}: {count} rows, logical sha256={source_checksum}",
            table.name
        );
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
        tables.len()
    );
    Ok(())
}

#[cfg(not(feature = "mariadb"))]
fn copy_to_mariadb(_args: &Args, _counts: &[TableCount]) -> Result<(), Box<dyn std::error::Error>> {
    Err("copy mode requires a build with --features mariadb".into())
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let tables = inspect_sqlite(&args.from_sqlite)?;
    println!("SQLite source: {}", args.from_sqlite.display());
    println!("Tables: {}", tables.len());
    for table in &tables {
        println!("  {}: {} rows", table.name, table.rows);
    }
    if args.dry_run {
        println!("MariaDB target: supplied via --to-dsn (value redacted)");
        println!("Plan:");
        println!("  1. Verify that the MariaDB target has no user tables.");
        println!("  2. Translate the SQLite table and index inventory to InnoDB.");
        println!("  3. Copy rows in table transactions.");
        println!("  4. Verify every row count and a logical SHA-256 for every table.");
        println!("Dry run only: MariaDB was not contacted and no data was changed.");
        return Ok(());
    }
    copy_to_mariadb(&args, &tables)
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

    fn fixture() -> PathBuf {
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
            "CREATE TABLE alpha(id INTEGER PRIMARY KEY, held TEXT DEFAULT 'ready');
             CREATE TABLE beta(value TEXT NOT NULL);
             INSERT INTO alpha VALUES(1, 'one');
             INSERT INTO beta VALUES('one'),('two');",
        )
        .unwrap();
        drop(db);
        path
    }

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
    fn force_is_explicit_and_not_a_dry_run_option() {
        let values = [
            "--from-sqlite",
            "state.sqlite",
            "--to-dsn",
            "mysql://example.invalid/podmesh-node",
            "--force",
        ]
        .map(String::from);
        assert!(parse_args(&values).unwrap().force);

        let mut invalid = values.to_vec();
        invalid.push("--dry-run".into());
        assert_eq!(
            parse_args(&invalid).unwrap_err(),
            "--force cannot be combined with --dry-run"
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
        let path = fixture();
        let result = inspect_sqlite(&path).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(
            result,
            vec![
                TableCount {
                    name: "alpha".into(),
                    rows: 1,
                },
                TableCount {
                    name: "beta".into(),
                    rows: 2,
                }
            ]
        );
    }

    #[cfg(feature = "mariadb")]
    #[test]
    fn translates_open_state_shapes_to_innodb() {
        let path = fixture();
        let db = open_source(&path).unwrap();
        let counts = inspect_sqlite(&path).unwrap();
        let tables = load_schema(&db, &counts).unwrap();
        let alpha = create_table_sql(&tables[0]).unwrap();
        assert!(alpha.contains("`id` BIGINT NOT NULL AUTO_INCREMENT"));
        assert!(alpha.contains("`held` LONGTEXT NULL DEFAULT 'ready'"));
        assert!(alpha.contains("PRIMARY KEY (`id`)"));
        assert!(alpha.contains("ENGINE=InnoDB"));
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(feature = "mariadb")]
    #[test]
    fn logical_checksum_ignores_row_order_but_not_value_kind() {
        let first = vec![
            vec![Value::Integer(1), Value::Text("one".into())],
            vec![Value::Integer(2), Value::Null],
        ];
        let reversed = first.iter().cloned().rev().collect::<Vec<_>>();
        assert_eq!(logical_checksum(&first), logical_checksum(&reversed));
        assert_ne!(
            logical_checksum(&first),
            logical_checksum(&[vec![Value::Text("1".into()), Value::Text("one".into())]])
        );
    }
}
