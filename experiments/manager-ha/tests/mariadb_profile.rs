//! Store-profile entry path on MariaDB when `PODMESH_MARIADB_DSN` names a lab sidecar.
#![cfg(feature = "mariadb")]
use podmesh::store::{Engine, MariadbConfig, StoreConfig};
use podmesh_manager_ha_lab::{
    durable::{Request, Response},
    ConfiguredStore, ReplicaConfig, ScopeGrant,
};
use podmesh_manager_ha_lab::durable::Configuration;

#[test]
fn a_mariadb_profile_executes_observe_without_a_sqlite_file() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "podmesh-manager-ha-mariadb-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sqlite_path = dir.join("manager.sqlite");
    let profile = StoreConfig {
        engine: Engine::Mariadb,
        mariadb,
        ..StoreConfig::for_manager_sqlite_path(&sqlite_path)
    };
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

    let mut store =
        ConfiguredStore::open(&profile, &sqlite_path, configuration, "r1").expect("profile opens");
    assert!(!sqlite_path.exists(), "MariaDB profile must not create the sqlite file");

    let response = store
        .execute(&Request::Observe {
            operation_id: "op-observe-1".into(),
            scope: "scope1".into(),
            subject: "subject-a".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "value-a".into(),
        })
        .expect("observe commits");
    match response {
        Response::Observed { fact } => assert_eq!(fact.subject, "subject-a"),
        other => panic!("unexpected response: {other:?}"),
    }

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
