//! `--inspect-store --facts-only`: the facts of a store, verified and printed
//! without its receipts or audit rows, so that neither the output nor the
//! verification grows with the exchange audit table.
use podmesh_manager_ha_lab::{
    durable::{
        AuditDirection, AuditOutcome, AuditPhase, Configuration as Manager, ExchangeAuditEvent,
        Request, Store,
    },
    ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::{ConfigurationFile, Peer};
use podmesh_manager_resident_lab::Configuration;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Lab {
    directory: tempfile::TempDir,
    config: Configuration,
}

impl Lab {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let manager = Manager {
            logical_manager_id: "inspection-test".into(),
            replicas: (0..2)
                .map(|i| ReplicaConfig {
                    replica_id: format!("r{i}"),
                    host_id: format!("h{i}"),
                })
                .collect(),
            grants: (0..2)
                .map(|i| ScopeGrant {
                    scope: format!("s{i}"),
                    owner_replica_id: format!("r{i}"),
                })
                .collect(),
        };
        let config = Configuration {
            network: ConfigurationFile {
                replica_id: "r0".into(),
                database_path: directory.path().join("r0.sqlite"),
                manager,
                bind: "127.0.0.1:9".parse().unwrap(),
                peers: vec![Peer {
                    replica_id: "r1".into(),
                    endpoint: "127.0.0.1:10".parse().unwrap(),
                    shared_key_hex: "01".repeat(32),
                }],
            },
            control_socket: directory.path().join("r0.sock"),
            observation_writer_uid: rustix::process::geteuid().as_raw(),
            interval_ms: 1_000,
            max_backoff_ms: 2_000,
            incoming_workers: 1,
            full_verification_interval_ms: None,
            unchanged_snapshot_refresh_ms: None,
            catch_up_window_ms: None,
            votes: None,
        };
        let path = directory.path().join("inspect.json");
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        Self { directory, config }
    }

    fn database(&self) -> &Path {
        &self.config.network.database_path
    }

    fn inspect(&self, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
            .env_remove("PODMESH_MANAGER_NETWORK_MODE")
            .arg("--inspect-store")
            .args(extra)
            .arg("--config")
            .arg(self.directory.path().join("inspect.json"))
            .arg("--state-dir")
            .arg(self.directory.path())
            .output()
            .unwrap()
    }

    fn inspected(&self, extra: &[&str]) -> Value {
        let output = self.inspect(extra);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn store(&self) -> Store {
        let network = &self.config.network;
        Store::open(
            &network.database_path,
            network.manager.clone(),
            &network.replica_id,
        )
        .unwrap()
    }

    /// Runs SQL on the store without the Store, as an actor bypassing it would.
    fn sql(&self, statement: &str) {
        let output = Command::new("python3")
            .arg("-c")
            .arg("import sqlite3, sys\nconnection = sqlite3.connect(sys.argv[1], isolation_level=None)\nconnection.execute(sys.argv[2])\nconnection.close()")
            .arg(self.database())
            .arg(statement)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn source_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
    ["", "-wal", "-shm"]
        .into_iter()
        .filter_map(|suffix| {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(suffix);
            let candidate = PathBuf::from(candidate);
            candidate
                .exists()
                .then(|| (suffix.into(), fs::read(candidate).unwrap()))
        })
        .collect()
}

fn prepared(index: usize) -> ExchangeAuditEvent {
    ExchangeAuditEvent {
        audit_event_id: format!("audit-{index}"),
        attempt_id: format!("attempt:{index:064x}"),
        wire_nonce: format!("nonce-{index}"),
        direction: AuditDirection::Outbound,
        phase: AuditPhase::OutboundRequestPrepared,
        authenticated_peer_id: Some("r1".into()),
        peer_claim: Some("r1".into()),
        operation_id: Some(format!("operation-{index}")),
        request_frame_bytes: 0,
        request_announced_body_bytes: Some(128),
        request_sha256: Some(format!("{:064x}", 1)),
        reply_frame_bytes: 0,
        reply_announced_body_bytes: None,
        reply_sha256: None,
        outcome: AuditOutcome::Incomplete,
        error_category: None,
        reason_code: None,
        local_receipt_operation_id: None,
        local_receipt_sha256: None,
        remote_receipt_operation_id: None,
        remote_receipt_sha256: None,
        replayed: false,
    }
}

#[test]
fn facts_only_inspection_prints_the_verified_facts_of_the_full_inspection() {
    let lab = Lab::new();
    {
        let mut store = lab.store();
        for index in 0..2 {
            store
                .execute(&Request::Observe {
                    operation_id: format!("fact-{index}"),
                    scope: "s0".into(),
                    subject: format!("subject-{index}"),
                    exclusive_resource: None,
                    active_claim: false,
                    value: "value".into(),
                })
                .unwrap();
        }
        for index in 0..20 {
            store.record_exchange_audit(&prepared(index)).unwrap();
        }
    }
    let before = source_bytes(lab.database());
    let full = lab.inspected(&[]);
    let facts = lab.inspected(&["--facts-only"]);
    assert_eq!(before, source_bytes(lab.database()));

    let Value::Object(fields) = &facts else {
        panic!("the facts-only inspection is not an object: {facts}");
    };
    let mut keys: Vec<_> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["history_count", "logical_history_sha256", "ordered_facts"]
    );
    assert_eq!(facts["history_count"], 2);
    for field in ["history_count", "ordered_facts", "logical_history_sha256"] {
        assert_eq!(facts[field], full[field], "{field}");
    }
    assert_eq!(full["audit_event_count"], 20);

    // It reads no audit row: an audit row that the full inspection refuses
    // leaves the facts readable.
    lab.sql("INSERT INTO exchange_audit_events VALUES ('forged', 'attempt:forged', 'nonce', 'outbound', 'outbound_request_prepared', NULL, NULL, NULL, 0, NULL, NULL, 0, NULL, NULL, 'incomplete', NULL, NULL, NULL, NULL, NULL, NULL, 0, '{}', '')");
    assert!(!lab.inspect(&[]).status.success());
    assert_eq!(lab.inspected(&["--facts-only"]), facts);

    // It verifies every fact.
    lab.sql("INSERT INTO facts(event_id, fact_json, sha256) VALUES ('r0:00000000000000000003', '{}', 'not the checksum')");
    let refused = lab.inspect(&["--facts-only"]);
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("corrupt: stored fact hash mismatch"));
}

#[test]
fn facts_only_is_accepted_only_as_a_form_of_inspect_store() {
    let lab = Lab::new();
    lab.store();
    let binary = env!("CARGO_BIN_EXE_podmesh-manager-resident-lab");
    let config = lab.directory.path().join("inspect.json");
    let state = lab.directory.path();
    for flags in [
        vec!["--facts-only"],
        vec!["--facts-only", "--inspect-store", "--facts-only"],
        vec!["--facts-only", "--validate-config"],
    ] {
        let output = Command::new(binary)
            .env_remove("PODMESH_MANAGER_NETWORK_MODE")
            .args(flags.clone())
            .arg("--config")
            .arg(&config)
            .arg("--state-dir")
            .arg(state)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flags:?} was accepted");
        assert!(output.stdout.is_empty());
    }
    assert!(lab.inspect(&["--facts-only"]).status.success());
}
