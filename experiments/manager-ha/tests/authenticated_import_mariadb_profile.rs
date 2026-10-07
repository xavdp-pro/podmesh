//! MariaDB profile must expose authenticated import (not a silent SQLite-only path).
#![cfg(feature = "mariadb")]

use podmesh::store::{Engine, MariadbConfig, StoreConfig};
use podmesh_manager_ha_lab::{
    durable::{
        AuditDirection, AuditOutcome, AuditPhase, Configuration, DurableError, ExchangeAuditEvent,
        Snapshot,
    },
    ConfiguredStore, ReplicaConfig, ScopeGrant,
};

#[test]
fn mariadb_profile_execute_authenticated_import_rejects_inconsistent_audit() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "podmesh-manager-ha-auth-import-{}",
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
        replicas: vec![
            ReplicaConfig {
                replica_id: "r1".into(),
                host_id: "h1".into(),
            },
            ReplicaConfig {
                replica_id: "r2".into(),
                host_id: "h2".into(),
            },
        ],
        grants: vec![ScopeGrant {
            scope: "scope1".into(),
            owner_replica_id: "r1".into(),
        }],
    };
    let snapshot = Snapshot {
        configuration: configuration.clone(),
        replica_id: "r1".into(),
        facts: vec![],
    };
    let mut store =
        ConfiguredStore::open(&profile, &sqlite_path, configuration, "r2").expect("profile opens");
    let audit = ExchangeAuditEvent {
        audit_event_id: "audit-import".into(),
        attempt_id: "attempt".into(),
        wire_nonce: "nonce".into(),
        direction: AuditDirection::Inbound,
        phase: AuditPhase::InboundImportCommitted,
        authenticated_peer_id: Some("r1".into()),
        peer_claim: None,
        operation_id: Some("wire-op".into()),
        request_frame_bytes: 0,
        request_announced_body_bytes: None,
        request_sha256: None,
        reply_frame_bytes: 0,
        reply_announced_body_bytes: None,
        reply_sha256: None,
        outcome: AuditOutcome::Accepted,
        error_category: None,
        reason_code: None,
        local_receipt_operation_id: None,
        local_receipt_sha256: None,
        remote_receipt_operation_id: None,
        remote_receipt_sha256: None,
        replayed: false,
    };

    let problem = store
        .execute_authenticated_import("different-wire-op", &snapshot, audit)
        .expect_err("inconsistent wire operation must refuse before storage");
    assert!(
        matches!(problem, DurableError::InvalidAudit(_)),
        "expected invalid_audit, got {problem}"
    );

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
