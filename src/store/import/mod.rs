//! Bounded synthetic node import. Completion is data verification, never runtime admission.
pub mod canonical;
pub mod caps;
pub mod source;
pub mod snapshot;
pub mod fixture;

use super::{DurableStore, Fault, Result};

pub const MARKER: &str = "podmesh_import_state";
pub const CHECKPOINTS: &str = "podmesh_import_tables";

pub(crate) fn refusal(code: &str) -> super::StoreError {
    Fault::Denied.error(code)
}

/// Read-only normal-path gate, including direct schema helpers and backend callers.
/// An empty or malformed marker is already non-serving; do not interpret its contents.
pub fn guard_normal(store: &mut dyn DurableStore) -> Result<()> {
    let sql = match store.engine() {
        super::Engine::Sqlite => "SELECT name FROM sqlite_schema",
        super::Engine::Mariadb => "SELECT table_name FROM information_schema.tables WHERE table_schema=DATABASE()",
    };
    if store.query(sql, &[])?.iter().any(|row| row.text(0).map(|name| name.starts_with("podmesh_import_")).unwrap_or(true)) {
        return Err(refusal("import_non_serving: marked stores require separate explicit adoption"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{bootstrap, ensure_schema_table, migrations, SqliteStore};

    #[test]
    fn marker_only_blocks_all_public_schema_writes() {
        for create in [
            "CREATE TABLE podmesh_import_state(unrecognized TEXT);",
            "CREATE TABLE podmesh_import_tables(unrecognized TEXT);",
        ] {
            let mut db = SqliteStore::open_in_memory().unwrap();
            db.execute_batch(create).unwrap();
            let before = db.tables().unwrap();
            assert!(migrations::apply(&mut db).unwrap_err().message.contains("import_non_serving"));
            assert!(migrations::apply_set(&mut db, migrations::NODE, &migrations::NODE_MIGRATIONS).is_err());
            assert!(ensure_schema_table(&mut db).is_err());
            assert!(bootstrap(&mut db, "node", 11).is_err());
            assert_eq!(db.tables().unwrap(), before);
            assert!(!before.contains(&"store_schema".to_owned()));
        }
    }

    #[test]
    fn marker_view_is_not_a_normal_path_bypass() {
        let mut db = SqliteStore::open_in_memory().unwrap();
        db.execute_batch("CREATE VIEW podmesh_import_state AS SELECT 'COMPLETE' AS phase;").unwrap();
        assert!(migrations::apply(&mut db).is_err());
        assert!(db.tables().unwrap().is_empty());
    }

    #[test]
    fn complete_or_malformed_marker_never_becomes_admission() {
        for state in ["", "INCOMPLETE", "COPYING", "VERIFYING", "FAILED", "COMPLETE", "unknown"] {
            let mut db = SqliteStore::open_in_memory().unwrap();
            db.execute_batch("CREATE TABLE podmesh_import_state(phase TEXT);").unwrap();
            db.execute("INSERT INTO podmesh_import_state VALUES(?)", &[state.into()]).unwrap();
            assert!(migrations::apply(&mut db).is_err());
            assert_eq!(db.query("SELECT phase FROM podmesh_import_state", &[]).unwrap()[0].text(0).unwrap(), state);
            assert_eq!(db.tables().unwrap(), [MARKER]);
        }
    }
}
