//! Manager store open path: versioned schema and cutover refusal (Phase 3 slice).
use podmesh::store::{bootstrap, migrations, Engine, SqliteStore, StoreConfig};
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
