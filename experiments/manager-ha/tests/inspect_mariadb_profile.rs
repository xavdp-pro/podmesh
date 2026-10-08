//! `--inspect-store` through a MariaDB `store.json` profile (lab sidecar).
#![cfg(feature = "mariadb")]

use podmesh::store::{Engine, MariadbConfig, StoreConfig};
use podmesh_manager_ha_lab::durable::{
    inspect_facts_read_only_resolved, inspect_read_only_resolved, Configuration, Request,
};
use podmesh_manager_ha_lab::{ConfiguredStore, ReplicaConfig, ScopeGrant};

/// Run serially on a dedicated empty database. Refusal must not install the
/// manager schema, advance its version, or change the incomplete-cutover marker.
#[test]
fn refused_mariadb_inspection_does_not_bootstrap_or_upgrade() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sqlite_path = dir.path().join("manager.sqlite");
    std::fs::write(
        dir.path().join("store.json"),
        serde_json::to_vec(&serde_json::json!({
            "store": {"engine": "mariadb", "mariadb": {"dsn": mariadb.dsn.clone().unwrap()}}
        }))
        .unwrap(),
    )
    .unwrap();
    let profile = StoreConfig {
        engine: Engine::Mariadb,
        mariadb,
        ..StoreConfig::for_manager_sqlite_path(&sqlite_path)
    };
    let mut backend = podmesh::store::open(&profile).unwrap();
    assert!(
        backend.tables().unwrap().is_empty(),
        "test requires a dedicated empty database"
    );
    let configuration = Configuration {
        logical_manager_id: "inspection-refusal".into(),
        replicas: vec![ReplicaConfig {
            replica_id: "r1".into(),
            host_id: "h1".into(),
        }],
        grants: vec![],
    };
    for version in [
        None,
        Some(0),
        Some(podmesh::store::migrations::manager_version() + 1),
    ] {
        if let Some(version) = version {
            podmesh::store::bootstrap(
                backend.as_mut(),
                podmesh::store::migrations::MANAGER,
                version,
            )
            .unwrap();
        }
        let before = backend.tables().unwrap();
        let full = inspect_read_only_resolved(dir.path(), &sqlite_path, &configuration, "r1");
        let facts =
            inspect_facts_read_only_resolved(dir.path(), &sqlite_path, &configuration, "r1");
        assert!(full.is_err(), "unsupported store must be refused");
        assert!(
            facts.is_err(),
            "facts-only inspection must refuse the same unsupported store"
        );
        assert_eq!(
            backend.tables().unwrap(),
            before,
            "refused inspection changed table inventory"
        );
        if version.is_some() {
            assert_eq!(
                podmesh::store::schema_version(
                    backend.as_mut(),
                    podmesh::store::migrations::MANAGER
                )
                .unwrap(),
                version,
                "refused inspection advanced schema version"
            );
        }
        assert!(
            !sqlite_path.exists(),
            "inspection created a SQLite fallback"
        );
    }
    // Only this test's own schema table exists after the successful checks.
    backend.execute_batch("DROP TABLE store_schema;").unwrap();
    podmesh::store::migrations::apply_manager(backend.as_mut()).unwrap();
    podmesh::store::bootstrap(
        backend.as_mut(),
        podmesh::store::migrations::MANAGER_CUTOVER,
        podmesh::store::migrations::CUTOVER_INCOMPLETE,
    )
    .unwrap();
    for facts_only in [false, true] {
        let error = if facts_only {
            inspect_facts_read_only_resolved(dir.path(), &sqlite_path, &configuration, "r1")
                .unwrap_err()
        } else {
            inspect_read_only_resolved(dir.path(), &sqlite_path, &configuration, "r1").unwrap_err()
        };
        assert!(error.to_string().contains("incomplete"), "{error}");
        assert_eq!(
            podmesh::store::schema_version(
                backend.as_mut(),
                podmesh::store::migrations::MANAGER_CUTOVER
            )
            .unwrap(),
            Some(podmesh::store::migrations::CUTOVER_INCOMPLETE)
        );
    }
    for table in podmesh::store::migrations::manager_tables()
        .iter()
        .chain([podmesh::store::SCHEMA_TABLE].iter())
    {
        backend
            .execute_batch(&format!("DROP TABLE `{table}`;"))
            .unwrap();
    }
    assert!(backend.tables().unwrap().is_empty());
}

#[test]
fn inspect_store_resolves_mariadb_profile_without_sqlite_file() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "podmesh-manager-inspect-mariadb-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sqlite_path = dir.join("manager.sqlite");
    let dsn = mariadb
        .dsn
        .clone()
        .expect("lab profile uses PODMESH_MARIADB_DSN");
    let store_json = serde_json::json!({
        "store": {
            "engine": "mariadb",
            "mariadb": { "dsn": dsn }
        }
    });
    std::fs::write(dir.join("store.json"), serde_json::to_vec(&store_json).unwrap()).unwrap();

    let configuration = Configuration {
        logical_manager_id: "manager".into(),
        replicas: vec![ReplicaConfig {
            replica_id: "r1".into(),
            host_id: "h1".into(),
        }],
        grants: vec![ScopeGrant {
            scope: "scope1".into(),
            owner_replica_id: "r1".into(),
        }],
    };

    let mut store = ConfiguredStore::open_resolved(&dir, &sqlite_path, configuration.clone(), "r1")
        .expect("profile opens");
    store
        .execute(&Request::Observe {
            operation_id: "inspect-mariadb-1".into(),
            scope: "scope1".into(),
            subject: "subject-a".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "value-a".into(),
        })
        .expect("observe commits");

    let inspection = inspect_read_only_resolved(&dir, &sqlite_path, &configuration, "r1")
        .expect("inspect-store on MariaDB profile");
    assert_eq!(inspection.schema_version, 3);
    assert_eq!(inspection.sqlite_integrity_result, "ok");
    assert_eq!(inspection.history_count, 1);
    assert!(
        !sqlite_path.exists(),
        "MariaDB profile must not require the sqlite file"
    );

    let profile = StoreConfig {
        engine: Engine::Mariadb,
        mariadb,
        ..StoreConfig::for_manager_sqlite_path(&sqlite_path)
    };
    let mut backend = podmesh::store::open(&profile).unwrap();
    for table in podmesh::store::migrations::manager_tables()
        .iter()
        .chain([podmesh::store::SCHEMA_TABLE].iter())
    {
        backend
            .execute_batch(&format!("DROP TABLE IF EXISTS `{table}`;"))
            .unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
