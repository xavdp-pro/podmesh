//! What a store carries, asked once here instead of in every caller's dialect.
//!
//! Two questions are asked all over this tree: *does this journal have that table*, and *does that
//! table have that column*. Before Phase 2 each caller answered them in SQLite's own words --
//! `SELECT ... FROM sqlite_master`, `PRAGMA table_info`, `pragma_table_info(...)` -- which is
//! exactly the introspection the migration plan lists as a risk, because MariaDB has neither.
//!
//! This module answers them for both engines: `sqlite_master` and `pragma_table_info` on one
//! side, `information_schema` on the other. A caller on the migrated path asks [`has_table`] and
//! [`has_column`] and never names a catalog.
//!
//! ## The SQLite-only escapes that remain
//!
//! Two callers still read SQLite's own catalog, and both are named here rather than left in the
//! modules:
//!
//! * [`connection::table_definition`] returns the `CREATE TABLE` text SQLite stored, which is how
//!   `network::ensure_schema` recognises a `network_allocations` from the first two versions and
//!   rebuilds it. There is no portable form: the check is "was this table declared with a
//!   uniqueness the current schema does not want", and only SQLite keeps the declaration verbatim.
//!   The migration set creates the current shape, so a MariaDB store is never in the state this
//!   escape repairs, and the offline `podmesh-storage-migrate` copies rows rather than DDL.
//! * `tests/check-package-rehearsal.py` lists tables from `sqlite_master` while comparing a
//!   journal before and after a package operation. It reads the file directly, outside the node.
//!
//! Everything else that used to read a catalog now goes through this module.
use super::config::Engine;
use super::{DurableStore, Fault, Result, Value};

/// The base tables the store carries, in name order.
pub fn tables(store: &mut dyn DurableStore) -> Result<Vec<String>> {
    store.tables()
}

pub fn has_table(store: &mut dyn DurableStore, table: &str) -> Result<bool> {
    Ok(store.tables()?.iter().any(|carried| carried == table))
}

/// The columns of a table, in the order the store declares them; empty for a table it lacks.
pub fn columns(store: &mut dyn DurableStore, table: &str) -> Result<Vec<String>> {
    let rows = match store.engine() {
        // `pragma_table_info` is a table-valued function, and its argument is bound like any
        // other: no name is ever pasted into the statement.
        Engine::Sqlite => store.query("SELECT name FROM pragma_table_info(?)", &[Value::from(table)])?,
        Engine::Mariadb => store.query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = ? ORDER BY ordinal_position",
            &[Value::from(table)],
        )?,
    };
    rows.iter().map(|row| row.text(0).map(str::to_string)).collect()
}

pub fn has_column(store: &mut dyn DurableStore, table: &str, column: &str) -> Result<bool> {
    Ok(columns(store, table)?.iter().any(|carried| carried == column))
}

/// The version of the same questions for a caller that still holds the `rusqlite` connection
/// `open_state` returns. They are the same statements [`columns`] runs on the SQLite side, in one
/// place, so that moving such a caller onto [`DurableStore`] changes no behaviour.
pub mod connection {
    use rusqlite::{Connection, OptionalExtension};

    pub fn has_table(db: &Connection, table: &str) -> rusqlite::Result<bool> {
        Ok(db
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn columns(db: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
        let mut statement = db.prepare("SELECT name FROM pragma_table_info(?1)")?;
        let names = statement.query_map([table], |row| row.get::<_, String>(0))?;
        names.collect()
    }

    pub fn has_column(db: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
        db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |row| Ok(row.get::<_, i64>(0)? > 0),
        )
    }

    /// **SQLite-only escape.** The `CREATE TABLE` text this engine kept for a table, which is the
    /// only way to ask what a table was *declared* with rather than what it holds. MariaDB keeps
    /// no such text, and a store the migration set made never needs the question. See this
    /// module's own documentation for why the one caller that asks it cannot be made portable.
    pub fn table_definition(db: &Connection, table: &str) -> rusqlite::Result<Option<String>> {
        db.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |row| row.get(0))
            .optional()
    }
}

/// A store that carries none of the node's tables is not a node journal, said once rather than as
/// a missing-table fault from whichever statement happened to run first.
pub fn require_tables(store: &mut dyn DurableStore, required: &[&str]) -> Result<()> {
    let carried = store.tables()?;
    let missing: Vec<&str> = required.iter().copied().filter(|table| !carried.iter().any(|name| name == table)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(Fault::Schema.error(format!("the store carries no {}", missing.join(", "))))
}

#[cfg(test)]
mod tests {
    use super::super::{migrations, sqlite::SqliteStore};
    use super::*;

    #[test]
    fn a_migrated_store_answers_what_it_carries_without_a_dialect() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        assert!(!has_table(&mut store, "secrets").unwrap());
        migrations::apply(&mut store).unwrap();

        assert!(has_table(&mut store, "secrets").unwrap());
        assert!(!has_table(&mut store, "manager_history").unwrap());
        assert_eq!(
            columns(&mut store, "secrets").unwrap(),
            ["name", "sha256", "bytes", "declared_at", "operation_id", "authorization_ref", "removed_at", "state"]
        );
        assert!(has_column(&mut store, "secrets", "state").unwrap());
        assert!(!has_column(&mut store, "secrets", "content").unwrap());
        assert!(columns(&mut store, "manager_history").unwrap().is_empty());
        assert!(tables(&mut store).unwrap().contains(&"operations".to_string()));

        require_tables(&mut store, &["metadata", "operations"]).unwrap();
        let refused = require_tables(&mut store, &["metadata", "manager_history"]).unwrap_err();
        assert_eq!(refused.fault, Fault::Schema);
        assert!(refused.message.contains("manager_history"));
    }

    /// The connection-shaped helpers answer the same, because the callers that still hold a
    /// `rusqlite::Connection` must not drift from the ones that hold a store.
    #[test]
    fn the_connection_helpers_answer_the_same_as_the_store_ones() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        migrations::apply(&mut store).unwrap();
        let carried = columns(&mut store, "network_routes").unwrap();
        let db = store.into_connection();
        assert_eq!(connection::columns(&db, "network_routes").unwrap(), carried);
        assert!(connection::has_table(&db, "network_routes").unwrap());
        assert!(!connection::has_table(&db, "network_history").unwrap());
        assert!(connection::has_column(&db, "network_routes", "state").unwrap());
        assert!(!connection::has_column(&db, "network_routes", "metric").unwrap());
        assert!(connection::table_definition(&db, "network_routes").unwrap().unwrap().contains("network_routes"));
        assert_eq!(connection::table_definition(&db, "network_history").unwrap(), None);
    }
}
