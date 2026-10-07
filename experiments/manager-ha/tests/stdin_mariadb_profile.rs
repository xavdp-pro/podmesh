//! Stdin JSON requests through a MariaDB `store.json` profile (lab sidecar).
#![cfg(feature = "mariadb")]

use std::{
    io::Write,
    process::{Command, Stdio},
};

use podmesh::store::{Engine, MariadbConfig, StoreConfig};
use podmesh_manager_ha_lab::durable::{Configuration, Request, Response};
use podmesh_manager_ha_lab::{ReplicaConfig, ScopeGrant};

#[test]
fn stdin_observe_resolves_mariadb_profile_without_sqlite_file() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "podmesh-manager-stdin-mariadb-{}",
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
    let config_path = dir.join("configuration.json");
    std::fs::write(&config_path, serde_json::to_vec(&configuration).unwrap()).unwrap();

    let request = Request::Observe {
        operation_id: "stdin-mariadb-1".into(),
        scope: "scope1".into(),
        subject: "subject-a".into(),
        exclusive_resource: None,
        active_claim: false,
        value: "value-a".into(),
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_podmesh-manager-ha-lab"))
        .args([
            sqlite_path.as_os_str(),
            config_path.as_os_str(),
            "r1".as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Response = serde_json::from_slice(&output.stdout).unwrap();
    match response {
        Response::Observed { fact } => assert_eq!(fact.subject, "subject-a"),
        other => panic!("unexpected response: {other:?}"),
    }
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
