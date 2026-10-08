//! Network entry must honor the same explicit profile as the resident.
use podmesh_manager_ha_lab::{durable::Configuration, ReplicaConfig, ScopeGrant};
use podmesh_manager_network_lab::{ConfigurationFile, Peer};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

fn configuration(directory: &Path, replica: usize, endpoint: SocketAddr) -> ConfigurationFile {
    ConfigurationFile {
        replica_id: format!("r{replica}"),
        database_path: directory.join("manager.sqlite"),
        manager: Configuration {
            logical_manager_id: "private-mariadb-network".into(),
            replicas: (1..=2)
                .map(|n| ReplicaConfig {
                    replica_id: format!("r{n}"),
                    host_id: format!("h{n}"),
                })
                .collect(),
            grants: (1..=2)
                .map(|n| ScopeGrant {
                    scope: format!("scope{n}"),
                    owner_replica_id: format!("r{n}"),
                })
                .collect(),
        },
        bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        peers: vec![Peer {
            replica_id: format!("r{}", 3 - replica),
            endpoint,
            shared_key_hex: "11".repeat(32),
        }],
    }
}

fn no_sqlite(directory: &Path) {
    for name in ["manager.sqlite", "manager.sqlite-wal", "manager.sqlite-shm"] {
        assert!(
            !directory.join(name).exists(),
            "unexpected SQLite side journal"
        );
    }
}

#[test]
fn invalid_explicit_profile_refuses_without_creating_sqlite() {
    assert!(
        std::env::var_os("PODMESH_STORE_PROFILE").is_none(),
        "test requires per-replica profiles"
    );
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("store.json"),
        br#"{"store":{"engine":"invalid"}}"#,
    )
    .unwrap();
    assert!(configuration(
        directory.path(),
        1,
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9))
    )
    .open()
    .is_err());
    no_sqlite(directory.path());
}

#[cfg(feature = "mariadb")]
#[test]
fn unavailable_explicit_mariadb_profile_refuses_without_sqlite_fallback() {
    assert!(std::env::var_os("PODMESH_STORE_PROFILE").is_none());
    let directory = tempfile::tempdir().unwrap();
    // Select an unused ephemeral endpoint; no database is provisioned here.
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let dsn = format!(
        "mysql://fixture:fixture@127.0.0.1:{}/unavailable-test",
        endpoint.port()
    );
    podmesh::store::MariadbConfig::from_dsn(dsn.clone())
        .validate()
        .unwrap();
    let profile = serde_json::json!({"store":{"engine":"mariadb","mariadb":{"dsn":dsn,"connect_timeout_ms":100}}});
    std::fs::write(
        directory.path().join("store.json"),
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();
    let resolved = podmesh::resolve_manager_store_profile(
        directory.path(),
        &directory.path().join("manager.sqlite"),
    )
    .unwrap();
    resolved.mariadb.validate().unwrap();
    let result = configuration(
        directory.path(),
        1,
        SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
    )
    .open();
    let problem = match result {
        Ok(_) => panic!("unavailable MariaDB must refuse"),
        Err(problem) => problem,
    };
    assert!(
        problem.to_string().contains("could not be opened"),
        "must reach the MariaDB connection, not profile validation"
    );
    no_sqlite(directory.path());
}

#[cfg(feature = "mariadb")]
#[test]
fn real_private_mariadb_connect_refusal_persists_terminal_after_reopen() {
    use podmesh::store::{self, Engine, MariadbConfig, StoreConfig};
    use podmesh_manager_ha_lab::durable::{AuditOutcome, AuditPhase, RefusalReason};
    use podmesh_manager_network_lab::{ErrorCategory, ErrorSource};
    use std::{io::Write, net::TcpListener, os::unix::fs::OpenOptionsExt};

    assert!(std::env::var_os("PODMESH_STORE_PROFILE").is_none());
    let dsn = std::env::var("PODMESH_MARIADB_DSN").expect("fresh private MariaDB required");
    let profile = StoreConfig {
        engine: Engine::Mariadb,
        mariadb: MariadbConfig::from_dsn(dsn.clone()),
        ..StoreConfig::default()
    };
    let mut raw = store::open(&profile).unwrap();
    assert!(raw.query(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE()", &[]
    ).unwrap().is_empty(), "fresh private database required");
    drop(raw);
    let directory = tempfile::tempdir().unwrap();
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(directory.path().join("store.json")).unwrap();
    file.write_all(serde_json::to_string(&serde_json::json!({
        "store":{"engine":"mariadb","mariadb":{"dsn":dsn}}
    })).unwrap().as_bytes()).unwrap();
    drop(file);
    // Reserve an ephemeral TCP endpoint, then close it: the real connect must fail.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let config = configuration(directory.path(), 1, endpoint);
    let mut node = config.open().unwrap();
    let problem = node.sync_to("r2", "connect-refused", "connect-refused-nonce").unwrap_err();
    assert_eq!(problem.category(), ErrorCategory::Unavailable);
    assert_eq!(problem.source(), ErrorSource::Local);
    assert!(problem.connection_attempted());
    drop(node);
    // Reopen the actual private store; an error category alone cannot prove its terminal.
    drop(config.open().unwrap());
    let audit = podmesh_manager_ha_lab::durable::inspect_profile::inspect_read_only_resolved(
        directory.path(), &config.database_path, &config.manager, &config.replica_id
    ).unwrap();
    assert_eq!(audit.audit_event_count, 2);
    assert!(audit.incomplete_attempts.is_empty());
    let terminal = audit.ordered_audit_events.iter().find(|evidence| {
        evidence.event.phase == AuditPhase::OutboundExchangeCompleted
    }).unwrap();
    assert_eq!(terminal.event.outcome, AuditOutcome::Unavailable);
    assert_eq!(terminal.event.reason_code, Some(RefusalReason::TransportUnavailable));
    assert!(terminal.event.authenticated_peer_id.is_none());
    assert_eq!(terminal.event.request_frame_bytes, 0);
    assert_eq!(terminal.event.reply_frame_bytes, 0);
    let mut raw = store::open(&profile).unwrap();
    let row = raw.query_one(
        "SELECT outcome,error_category,reason_code,request_frame_bytes,reply_frame_bytes FROM exchange_audit_events WHERE phase='outbound_exchange_completed'", &[]
    ).unwrap().unwrap();
    assert_eq!(row.text(0).unwrap(), "unavailable");
    assert_eq!(row.text(1).unwrap(), "unavailable");
    assert_eq!(row.text(2).unwrap(), "transport_unavailable");
    assert_eq!(row.integer(3).unwrap(), 0);
    assert_eq!(row.integer(4).unwrap(), 0);
    no_sqlite(directory.path());
}

#[cfg(feature = "mariadb")]
#[test]
fn real_private_mariadb_authenticated_exchange_has_one_journal_and_no_sqlite() {
    use podmesh::store::{self, Engine, MariadbConfig, StoreConfig};
    use podmesh_manager_ha_lab::{durable::Request, ConfiguredStore};
    use std::{io::Write, net::TcpListener, os::unix::fs::OpenOptionsExt, thread};

    assert!(
        std::env::var_os("PODMESH_STORE_PROFILE").is_none(),
        "test requires per-replica profiles"
    );
    // Deliberately require both DSNs: a skipped fixture cannot prove this regression.
    let dsns = [
        std::env::var("PODMESH_MARIADB_DSN").expect("fresh first private MariaDB required"),
        std::env::var("PODMESH_MARIADB_PEER_DSN").expect("fresh second private MariaDB required"),
    ];
    assert!(dsns[0] != dsns[1], "two distinct private servers required");
    let directories = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut profiles = Vec::new();
    for (directory, dsn) in directories.iter().zip(&dsns) {
        let profile = StoreConfig {
            engine: Engine::Mariadb,
            mariadb: MariadbConfig::from_dsn(dsn.clone()),
            ..StoreConfig::default()
        };
        let mut raw = store::open(&profile).unwrap();
        assert!(
            raw.query(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE()",
                &[]
            )
            .unwrap()
            .is_empty(),
            "fresh private database required"
        );
        drop(raw);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.path().join("store.json"))
            .unwrap();
        file.write_all(
            serde_json::to_string(
                &serde_json::json!({"store":{"engine":"mariadb","mariadb":{"dsn":dsn}}}),
            )
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        profiles.push(profile);
    }
    let unused = SocketAddr::from((Ipv4Addr::LOCALHOST, 9));
    for (index, directory) in directories.iter().enumerate() {
        let config = configuration(directory.path(), index + 1, unused);
        ConfiguredStore::open_resolved(
            directory.path(),
            &config.database_path,
            config.manager.clone(),
            &config.replica_id,
        )
        .unwrap()
        .execute(&Request::Observe {
            operation_id: format!("seed-{}", index + 1),
            scope: format!("scope{}", index + 1),
            subject: "private-journal".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "local fact".into(),
        })
        .unwrap();
    }
    // Both directions plus a same-operation retry with a distinct authenticated nonce.
    for (source, target, operation, nonce) in [
        (0, 1, "sync-1", "nonce-1"),
        (0, 1, "sync-1", "nonce-3"),
        (1, 0, "sync-2", "nonce-2"),
    ] {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut sender = configuration(
            directories[source].path(),
            source + 1,
            listener.local_addr().unwrap(),
        )
        .open()
        .unwrap();
        let mut receiver = configuration(directories[target].path(), target + 1, unused)
            .open()
            .unwrap();
        let serving = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            receiver
                .serve_connection(stream)
                .map_err(|error| error.to_string())
        });
        let result = sender
            .sync_to(&format!("r{}", target + 1), operation, nonce)
            .unwrap();
        assert_eq!(result.history_len, 2);
        assert_eq!(result.replayed, nonce == "nonce-3");
        serving.join().unwrap().unwrap();
    }
    let mut history_hashes = Vec::new();
    for (index, profile) in profiles.iter().enumerate() {
        let config = configuration(directories[index].path(), index + 1, unused);
        let mut journal = ConfiguredStore::open_resolved(
            directories[index].path(),
            &config.database_path,
            config.manager.clone(),
            &config.replica_id,
        )
        .unwrap();
        journal.verify_full().unwrap();
        let inspection =
            podmesh_manager_ha_lab::durable::inspect_profile::inspect_read_only_resolved(
                directories[index].path(),
                &config.database_path,
                &config.manager,
                &config.replica_id,
            )
            .unwrap();
        assert_eq!(inspection.history_count, 2);
        assert!(inspection.incomplete_attempts.is_empty());
        assert!(inspection.unaudited_import_receipt_ids.is_empty());
        history_hashes.push(inspection.logical_history_sha256);
        let mut raw = store::open(profile).unwrap();
        assert_eq!(
            raw.query_one("SELECT COUNT(*) FROM facts", &[])
                .unwrap()
                .unwrap()
                .integer(0)
                .unwrap(),
            2
        );
        assert!(
            raw.query_one("SELECT COUNT(*) FROM exchange_audit_events", &[])
                .unwrap()
                .unwrap()
                .integer(0)
                .unwrap()
                > 0
        );
        assert!(raw.query_one("SELECT COUNT(*) FROM exchange_audit_events a JOIN receipts r ON a.local_receipt_operation_id = r.operation_id AND a.local_receipt_sha256 = r.sha256 WHERE a.phase = 'inbound_import_committed'", &[]).unwrap().unwrap().integer(0).unwrap() > 0);
        no_sqlite(directories[index].path());
    }
    assert_eq!(history_hashes[0], history_hashes[1]);
}
