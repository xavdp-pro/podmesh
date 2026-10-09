//! MariaDB journal profile: `verify_full` and `integrity` parity with SQLite store.
#![cfg(feature = "mariadb")]
use podmesh::store::{Engine, MariadbConfig, StoreConfig};
use podmesh_manager_ha_lab::{
    durable::{Request, Response},
    ConfiguredStore, ReplicaConfig, ScopeGrant,
};
use podmesh_manager_ha_lab::durable::Configuration;

#[test]
fn mariadb_profile_verify_full_and_integrity_after_open() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "podmesh-manager-ha-integrity-mariadb-{}",
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
    let after_open = store.integrity().expect("integrity readable");
    assert!(
        after_open.full_verifications >= 1,
        "open runs a complete verification: {}",
        after_open.full_verifications
    );
    assert!(after_open.failure.is_none());
    assert!(after_open.last_full_verification_age.is_some());

    let response = store
        .execute(&Request::Observe {
            operation_id: "op-integrity-mariadb-1".into(),
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

    store.verify_full().expect("second full verification");
    let after_pass = store.integrity().expect("integrity readable");
    assert_eq!(
        after_pass.full_verifications,
        after_open.full_verifications + 1,
        "verify_full counts another complete pass"
    );
    assert!(after_pass.failure.is_none());

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
