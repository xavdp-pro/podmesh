//! Whole-table transactional copy mechanics. No schema creation and no serving transition.
use super::{canonical, refusal, source::Table};
use crate::store::{DurableStore, Result, Row, Value};

fn exact(table: &Table, rows: Vec<Row>) -> Result<()> {
    if rows
        .iter()
        .any(|row| row.columns() != table.columns.as_slice())
    {
        return Err(refusal("target_column_order_mismatch"));
    }
    let actual: Vec<Vec<Value>> = rows.into_iter().map(Row::into_values).collect();
    // Compare full typed vectors independently from the canonical digest.
    if canonical::sorted_rows(&actual)? != canonical::sorted_rows(&table.rows)?
        || canonical::table_hash(&table.name, &table.columns, &actual)? != table.hash
    {
        return Err(refusal("target_typed_multiset_mismatch"));
    }
    Ok(())
}

pub(super) fn verify_table(store: &mut dyn DurableStore, table: &Table) -> Result<()> {
    exact(table, store.query(&table.select()?, &[])?)
}

fn checkpoint(table: &Table, rows: Vec<Row>) -> Result<bool> {
    if rows.is_empty() {
        return Ok(false);
    }
    if rows.len() != 1
        || rows[0].integer(0)?
            != i64::try_from(table.rows.len()).map_err(|_| refusal("resource_counter_overflow"))?
        || rows[0].text(1)? != table.hash
    {
        return Err(refusal("table_checkpoint_mismatch"));
    }
    Ok(true)
}

/// Data and checkpoint are committed in the same transaction. A committed checkpoint is
/// skipped only after independent exact readback; uncheckpointed rows are never erased.
pub(super) fn copy_table(
    store: &mut dyn DurableStore,
    table: &Table,
    warnings: bool,
    event: &mut dyn FnMut(&str) -> Result<()>,
    commit_gate: &mut dyn FnMut(&mut dyn crate::store::Transaction) -> Result<()>,
) -> Result<bool> {
    let progress = store.query(
        "SELECT row_count,canonical_sha256 FROM podmesh_import_tables WHERE table_name=?",
        &[table.name.clone().into()],
    )?;
    if checkpoint(table, progress)? {
        verify_table(store, table)?;
        return Ok(false);
    }
    let before = store.query(
        &format!(
            "SELECT COUNT(*) FROM {}",
            super::source::quoted(&table.name)?
        ),
        &[],
    )?;
    if before.len() != 1 || before[0].integer(0)? != 0 {
        return Err(refusal("pending_table_has_rows"));
    }
    let insert = table.insert()?;
    let mut tx = store.transaction()?;
    for row in &table.rows {
        if tx.execute(&insert, row)? != 1 {
            return Err(refusal("table_insert_count_mismatch"));
        }
        if warnings && !tx.query("SHOW WARNINGS", &[])?.is_empty() {
            return Err(refusal("target_sql_warning_refused"));
        }
        event(&format!("after_row_before_table_commit:{}", table.name))?;
    }
    exact(table, tx.query(&table.select()?, &[])?)?;
    tx.execute(
        "INSERT INTO podmesh_import_tables(table_name,row_count,canonical_sha256) VALUES(?,?,?)",
        &[
            table.name.clone().into(),
            i64::try_from(table.rows.len())
                .map_err(|_| refusal("resource_counter_overflow"))?
                .into(),
            table.hash.clone().into(),
        ],
    )?;
    if warnings && !tx.query("SHOW WARNINGS", &[])?.is_empty() {
        return Err(refusal("target_sql_warning_refused"));
    }
    event(&format!("before_table_commit:{}", table.name))?;
    commit_gate(tx.as_mut())?;
    tx.commit()?;
    event(&format!("after_table_commit_before_ack:{}", table.name))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SqliteStore;
    fn setup() -> (SqliteStore, Table) {
        let mut db = SqliteStore::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE podmesh_import_tables(table_name TEXT PRIMARY KEY,row_count INTEGER NOT NULL,canonical_sha256 TEXT NOT NULL); CREATE TABLE sample(id INTEGER PRIMARY KEY,t TEXT,n INTEGER);").unwrap();
        let columns = vec!["id".into(), "t".into(), "n".into()];
        let rows = vec![
            vec![(-10_i64).into(), "e\u{301}\0 ".into(), Value::Null],
            vec![0_i64.into(), "é".into(), i64::MIN.into()],
            vec![10_i64.into(), "A".into(), i64::MAX.into()],
        ];
        let hash = canonical::table_hash("sample", &columns, &rows).unwrap();
        (
            db,
            Table {
                name: "sample".into(),
                columns,
                rows,
                hash,
            },
        )
    }
    #[test]
    fn before_commit_rolls_back_data_and_progress_then_resume_twice_is_exact() {
        let (mut db, table) = setup();
        let mut stop = |point: &str| {
            if point == "before_table_commit:sample" {
                Err(refusal("offline_interruption"))
            } else {
                Ok(())
            }
        };
        assert!(copy_table(&mut db, &table, false, &mut stop, &mut |_| Ok(())).is_err());
        assert_eq!(
            db.query("SELECT COUNT(*) FROM sample", &[]).unwrap()[0]
                .integer(0)
                .unwrap(),
            0
        );
        assert!(db
            .query("SELECT * FROM podmesh_import_tables", &[])
            .unwrap()
            .is_empty());
        assert!(copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut |_| Ok(())).unwrap());
        assert!(!copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut |_| Ok(())).unwrap());
        assert!(!copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut |_| Ok(())).unwrap());
        verify_table(&mut db, &table).unwrap();
    }
    #[test]
    fn lost_ack_keeps_atomic_checkpoint_and_tampered_or_pending_rows_refuse() {
        let (mut db, table) = setup();
        let mut stop = |point: &str| {
            if point == "after_table_commit_before_ack:sample" {
                Err(refusal("offline_lost_ack"))
            } else {
                Ok(())
            }
        };
        assert!(copy_table(&mut db, &table, false, &mut stop, &mut |_| Ok(())).is_err());
        assert!(!copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut |_| Ok(())).unwrap());
        db.execute("UPDATE sample SET t=? WHERE id=0", &["wrong".into()])
            .unwrap();
        assert!(
            copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut |_| Ok(()))
                .unwrap_err()
                .message
                .contains("multiset")
        );
        db.execute("DELETE FROM podmesh_import_tables", &[])
            .unwrap();
        assert!(
            copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut |_| Ok(()))
                .unwrap_err()
                .message
                .contains("pending_table_has_rows")
        );
        assert_eq!(
            db.query("SELECT COUNT(*) FROM sample", &[]).unwrap()[0]
                .integer(0)
                .unwrap(),
            3
        );
    }
    #[test]
    fn failed_commit_durability_gate_rolls_back_data_and_checkpoint() {
        let (mut db, table) = setup();
        let mut denied =
            |_: &mut dyn crate::store::Transaction| Err(refusal("offline_changed_durability_gate"));
        assert!(copy_table(&mut db, &table, false, &mut |_| Ok(()), &mut denied).is_err());
        assert_eq!(
            db.query("SELECT COUNT(*) FROM sample", &[]).unwrap()[0]
                .integer(0)
                .unwrap(),
            0
        );
        assert!(db
            .query("SELECT * FROM podmesh_import_tables", &[])
            .unwrap()
            .is_empty());
    }
    #[test]
    fn all38_typed_synthetic_tables_move_through_real_offline_transactions() {
        // SQLite->SQLite copier mechanics, explicitly not MariaDB qualification.
        let root = std::env::temp_dir().join(format!(
            "podmesh-c02-copy38-{}-{}",
            std::process::id(),
            crate::now()
        ));
        let manifest = super::super::fixture::create(&root).unwrap();
        let caps = super::super::fixture::development_caps().unwrap();
        let original = root.join("source.sqlite");
        let capture =
            super::super::snapshot::capture(&original, &manifest, &root.join("snapshot"), &caps)
                .unwrap();
        let reader = super::super::snapshot::open_sealed(&capture.path, &capture.seal).unwrap();
        let plan = super::super::source::inspect(
            &reader,
            &caps,
            capture.seal["snapshot_bytes"].as_u64().unwrap(),
        )
        .unwrap();
        let mut target = SqliteStore::open_in_memory().unwrap();
        crate::store::migrations::apply_set(
            &mut target,
            crate::store::migrations::NODE,
            &crate::store::migrations::NODE_MIGRATIONS[..10],
        )
        .unwrap();
        target.execute_batch("CREATE TABLE podmesh_import_tables(table_name TEXT PRIMARY KEY,row_count INTEGER NOT NULL,canonical_sha256 TEXT NOT NULL);").unwrap();
        assert_eq!(plan.rows, 129);
        assert_eq!(plan.tables.len(), 38);
        for table in &plan.tables {
            assert!(
                copy_table(&mut target, table, false, &mut |_| Ok(()), &mut |_| Ok(())).unwrap()
            );
            verify_table(&mut target, table).unwrap();
        }
        for _ in 0..2 {
            for table in &plan.tables {
                assert!(
                    !copy_table(&mut target, table, false, &mut |_| Ok(()), &mut |_| Ok(()))
                        .unwrap()
                );
            }
        }
        assert_eq!(
            target
                .query("SELECT COUNT(*) FROM podmesh_import_tables", &[])
                .unwrap()[0]
                .integer(0)
                .unwrap(),
            38
        );
        assert_eq!(
            super::super::snapshot::bundle_manifest(&original, &caps).unwrap(),
            manifest["bundle"]
        );
        drop(reader);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn null_text_integer_blob_and_duplicate_multiplicity_never_coerce() {
        let (_, table) = setup();
        let row = Row::new(
            std::sync::Arc::new(table.columns.clone()),
            vec![0_i64.into(), Value::Blob(vec![195, 169]), i64::MIN.into()],
        );
        assert!(exact(&table, vec![row]).is_err());
        let mut duplicate = table.rows.clone();
        duplicate.push(table.rows[0].clone());
        let rows = duplicate
            .into_iter()
            .map(|v| Row::new(std::sync::Arc::new(table.columns.clone()), v))
            .collect();
        assert!(exact(&table, rows).is_err());
    }
}
