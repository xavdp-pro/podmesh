//! `--inspect-store` through a MariaDB `store.json` profile (lab sidecar).
#![cfg(feature = "mariadb")]

use podmesh::store::{Engine, MariadbConfig, StoreConfig};
use podmesh_manager_ha_lab::durable::{
    inspect_read_only_resolved, Configuration, Request,
};
use podmesh_manager_ha_lab::{ConfiguredStore, ReplicaConfig, ScopeGrant};

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
