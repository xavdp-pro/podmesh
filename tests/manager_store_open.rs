//! Manager store open path: versioned schema and cutover refusal (Phase 3 slice).
use podmesh::store::{bootstrap, migrations, Engine, SqliteStore, StoreConfig};
#[cfg(feature = "mariadb")]
use podmesh::store::MariadbConfig;
use std::{fs, path::PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("podmesh-manager-store-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_manager_sqlite_file_opens_at_the_current_schema_version() {
    let dir = scratch("sqlite");
    let path = dir.join("manager.sqlite");
    let profile = StoreConfig::for_manager_sqlite_path(&path);
    assert_eq!(profile.engine, Engine::Sqlite);

    let opened = podmesh::open_manager_store(&profile).unwrap();
    assert!(path.exists());
    let db = opened.into_connection().unwrap();
    let version: i64 = db
        .query_row(
            "SELECT version FROM store_schema WHERE name = 'manager'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, migrations::manager_version());
}

#[test]
fn an_incomplete_manager_cutover_refuses_to_open() {
    let dir = scratch("cutover");
    let path = dir.join("manager.sqlite");
    let profile = StoreConfig::for_manager_sqlite_path(&path);
    let mut store = SqliteStore::open(&profile.sqlite).unwrap();
    migrations::apply_manager(&mut store).unwrap();
    bootstrap(&mut store, migrations::MANAGER_CUTOVER, migrations::CUTOVER_INCOMPLETE).unwrap();
    drop(store.into_connection());

    let refused = podmesh::open_manager_store(&profile)
        .err()
        .expect("incomplete cutover opens nothing")
        .to_string();
    assert!(refused.contains("incomplete"), "{refused}");
}

#[test]
fn a_mariadb_profile_never_falls_back_to_a_file() {
    let path = scratch("mariadb").join("manager.sqlite");
    let incomplete = StoreConfig {
        engine: Engine::Mariadb,
        ..StoreConfig::for_manager_sqlite_path(&path)
    };
    let refused = podmesh::open_manager_store(&incomplete)
        .err()
        .expect("an incomplete MariaDB profile opens nothing")
        .to_string();
    assert!(refused.contains("password_file"), "{refused}");
    assert!(!path.exists(), "an unopened MariaDB profile made a file");
}

/// MariaDB manager journal opens through the profile and is handed to `manager-ha` as a durable
/// journal, while legacy `into_connection` still refuses.
#[cfg(feature = "mariadb")]
#[test]
fn a_mariadb_manager_journal_opens_and_exposes_the_durable_backend() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = scratch("mariadb-server");
    let config = StoreConfig {
        engine: Engine::Mariadb,
        mariadb,
        ..StoreConfig::for_manager_sqlite_path(&dir.join("manager.sqlite"))
    };

    let opened = podmesh::open_manager_store(&config).unwrap();
    assert_eq!(opened.engine(), Engine::Mariadb);
    match opened.into_journal() {
        podmesh::ManagerJournal::Durable(mut store) => {
            let version = podmesh::store::schema_version(store.as_mut(), migrations::MANAGER).unwrap();
            assert_eq!(version, Some(migrations::manager_version()));
        }
        podmesh::ManagerJournal::Sqlite(_) => panic!("MariaDB profile opened as SQLite"),
    }

    let refused = podmesh::open_manager_store(&config)
        .unwrap()
        .into_connection()
        .unwrap_err()
        .to_string();
    assert!(refused.contains("into_journal"), "{refused}");

    let mut store = podmesh::store::open(&config).unwrap();
    for table in migrations::manager_tables().iter().chain([podmesh::store::SCHEMA_TABLE].iter()) {
        store
            .execute_batch(&format!("DROP TABLE IF EXISTS `{table}`;"))
            .unwrap();
    }
    let _ = fs::remove_dir_all(&dir);
}
