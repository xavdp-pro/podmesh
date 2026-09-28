use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const USAGE: &str =
    "Usage: podmesh-storage-migrate --from-sqlite PATH --to-dsn URL [--dry-run]";

#[derive(Debug, PartialEq)]
struct Args {
    from_sqlite: PathBuf,
    to_dsn: String,
    dry_run: bool,
}

fn parse_args(values: &[String]) -> Result<Args, String> {
    let mut from_sqlite = None;
    let mut to_dsn = None;
    let mut dry_run = false;
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
            value => return Err(format!("unknown argument: {value}")),
        }
        index += 1;
    }

    Ok(Args {
        from_sqlite: from_sqlite.ok_or_else(|| "--from-sqlite is required".to_string())?,
        to_dsn: to_dsn.ok_or_else(|| "--to-dsn is required".to_string())?,
        dry_run,
    })
}

fn quoted_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn inspect_sqlite(path: &Path) -> Result<Vec<(String, i64)>, Box<dyn std::error::Error>> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
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
            let sql = format!("SELECT COUNT(*) FROM {}", quoted_identifier(&name));
            let count = db.query_row(&sql, [], |row| row.get(0))?;
            Ok((name, count))
        })
        .collect()
}

fn refuse_unimplemented_copy() -> Result<(), Box<dyn std::error::Error>> {
    // TODO(phase-2): connect to MariaDB, inspect information_schema, and proceed only
    // when the target database has no user tables. Copying and --force remain absent.
    Err(
        "MariaDB target inspection and copy are not implemented; refusing to modify the target"
            .into(),
    )
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if !args.dry_run {
        return refuse_unimplemented_copy();
    }

    let tables = inspect_sqlite(&args.from_sqlite)?;
    println!("SQLite source: {}", args.from_sqlite.display());
    println!("MariaDB target: supplied via --to-dsn (value redacted)");
    println!("Tables: {}", tables.len());
    for (table, rows) in &tables {
        println!("  {table}: {rows} rows");
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
    if values.iter().any(|value| value == "--help" || value == "-h") {
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
        assert_eq!(quoted_identifier("odd\"table"), "\"odd\"\"table\"");
    }
}
