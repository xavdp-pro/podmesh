//! Three executable replicas retain, replay, converge, and expose external evidence.

use std::{
    fs,
    io::{BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    process::{Child, Command, Output, Stdio},
};

use podmesh_manager_ha_lab::{
    durable::{AuditPhase, CanonicalStoreInspection, Configuration, Request, Store},
    ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::{ConfigurationFile, ImportResult, Peer};
use tempfile::TempDir;

fn key_for(left: usize, right: usize) -> String {
    match (left.min(right), left.max(right)) {
        (1, 2) => "11".repeat(32),
        (1, 3) => "22".repeat(32),
        (2, 3) => "33".repeat(32),
        _ => unreachable!(),
    }
}

fn manager() -> Configuration {
    Configuration {
        logical_manager_id: "process-manager".into(),
        replicas: (1..=3)
            .map(|number| ReplicaConfig {
                replica_id: format!("r{number}"),
                host_id: format!("h{number}"),
            })
            .collect(),
        grants: (1..=3)
            .map(|number| ScopeGrant {
                scope: format!("scope{number}"),
                owner_replica_id: format!("r{number}"),
            })
            .collect(),
    }
}

fn configuration(directory: &TempDir, replica_id: &str) -> ConfigurationFile {
    let replica_index: usize = replica_id[1..].parse().unwrap();
    ConfigurationFile {
        replica_id: replica_id.into(),
        database_path: directory.path().join(format!("{replica_id}.sqlite")),
        manager: manager(),
        bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        peers: (1..=3)
            .filter(|index| *index != replica_index)
            .map(|index| Peer {
                replica_id: format!("r{index}"),
                endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
                shared_key_hex: key_for(replica_index, index),
            })
            .collect(),
    }
}

fn write_configuration(directory: &TempDir, configuration: &ConfigurationFile) {
    fs::write(
        directory
            .path()
            .join(format!("{}.json", configuration.replica_id)),
        serde_json::to_vec(configuration).unwrap(),
    )
    .unwrap();
}

fn point_peer(configuration: &mut ConfigurationFile, peer_id: &str, endpoint: SocketAddr) {
    configuration
        .peers
        .iter_mut()
        .find(|peer| peer.replica_id == peer_id)
        .unwrap()
        .endpoint = endpoint;
}

fn spawn_ready_server(binary: &str, mode: &str, configuration_path: &Path) -> (Child, SocketAddr) {
    let mut command = Command::new(binary);
    command.args([mode, configuration_path.to_str().unwrap()]);
    if mode == "serve-once-drop-reply-ready" {
        command.env("PODMESH_MANAGER_NETWORK_LAB_ENABLE_REPLY_LOSS", "1");
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(!ready.is_empty(), "server exited without readiness");
    (child, ready.trim().parse().unwrap())
}

fn run_sync(
    binary: &str,
    configuration_path: &Path,
    peer: &str,
    operation: &str,
    nonce: &str,
) -> Output {
    Command::new(binary)
        .args([
            "sync",
            configuration_path.to_str().unwrap(),
            peer,
            operation,
            nonce,
        ])
        .output()
        .unwrap()
}

fn inspect_store(
    repository: &Path,
    database: &Path,
    manager_path: &Path,
    replica_id: &str,
) -> CanonicalStoreInspection {
    let output = Command::new("cargo")
        .current_dir(repository)
        .args([
            "run",
            "--quiet",
            "--locked",
            "--manifest-path",
            "experiments/manager-ha/Cargo.toml",
            "--",
            "--inspect-store",
            database.to_str().unwrap(),
            manager_path.to_str().unwrap(),
            replica_id,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn three_processes_retain_replay_and_converge_with_external_inspection() {
    let directory = tempfile::tempdir().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let binary = env!("CARGO_BIN_EXE_podmesh-manager-network-lab");
    let manager_path = directory.path().join("manager.json");
    fs::write(&manager_path, serde_json::to_vec(&manager()).unwrap()).unwrap();

    let mut r1 = configuration(&directory, "r1");
    let r2 = configuration(&directory, "r2");
    let r3 = configuration(&directory, "r3");
    for configuration in [&r1, &r2, &r3] {
        write_configuration(&directory, configuration);
    }
    let mut source = Store::open(&r1.database_path, r1.manager.clone(), "r1").unwrap();
    source
        .execute(&Request::Observe {
            operation_id: "origin-observe".into(),
            scope: "scope1".into(),
            subject: "universe".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "survives-reply-loss".into(),
        })
        .unwrap();
    drop(source);

    let r2_path = directory.path().join("r2.json");
    let r1_path = directory.path().join("r1.json");
    let (mut lost_reply_server, first_address) =
        spawn_ready_server(binary, "serve-once-drop-reply-ready", &r2_path);
    point_peer(&mut r1, "r2", first_address);
    write_configuration(&directory, &r1);
    let lost = run_sync(
        binary,
        &r1_path,
        "r2",
        "reply-loss-operation",
        "reply-loss-nonce-one",
    );
    assert!(!lost.status.success());
    assert!(!lost_reply_server.wait().unwrap().success());

    let (mut replay_server, replay_address) =
        spawn_ready_server(binary, "serve-once-ready", &r2_path);
    point_peer(&mut r1, "r2", replay_address);
    write_configuration(&directory, &r1);
    let replay = run_sync(
        binary,
        &r1_path,
        "r2",
        "reply-loss-operation",
        "reply-loss-nonce-two",
    );
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay_result: ImportResult = serde_json::from_slice(&replay.stdout).unwrap();
    assert!(replay_result.replayed);
    assert!(replay_server.wait().unwrap().success());

    let r3_path = directory.path().join("r3.json");
    let (mut r3_server, r3_address) = spawn_ready_server(binary, "serve-once-ready", &r3_path);
    point_peer(&mut r1, "r3", r3_address);
    write_configuration(&directory, &r1);
    let r3_sync = run_sync(binary, &r1_path, "r3", "catch-up-r3", "catch-up-r3-nonce");
    assert!(
        r3_sync.status.success(),
        "{}",
        String::from_utf8_lossy(&r3_sync.stderr)
    );
    assert!(r3_server.wait().unwrap().success());

    let inspections: Vec<_> = ["r1", "r2", "r3"]
        .into_iter()
        .map(|replica| {
            inspect_store(
                repository,
                &directory.path().join(format!("{replica}.sqlite")),
                &manager_path,
                replica,
            )
        })
        .collect();
    assert!(inspections.iter().all(|inspection| {
        inspection.sqlite_integrity_result == "ok"
            && inspection.history_count == 1
            && inspection.ordered_facts[0].value == "survives-reply-loss"
            && inspection.unaudited_import_receipt_ids.is_empty()
    }));
    assert!(inspections
        .windows(2)
        .all(|pair| pair[0].logical_history_sha256 == pair[1].logical_history_sha256));
    assert_eq!(
        inspections[0].logical_history_sha256,
        "96fd39c0393c8c621145688a91c9aa267cddadd7d51417eff0a87e435d96667c"
    );
    assert_eq!(inspections[0].audit_event_count, 5);
    assert_eq!(inspections[0].incomplete_attempts.len(), 1);
    assert_eq!(
        inspections[0].incomplete_attempts[0].last_phase,
        AuditPhase::OutboundRequestPrepared
    );
    assert_eq!(inspections[1].audit_event_count, 6);
    assert_eq!(inspections[1].incomplete_attempts.len(), 1);
    assert_eq!(
        inspections[1].incomplete_attempts[0].last_phase,
        AuditPhase::InboundImportCommitted
    );
    assert_eq!(inspections[2].audit_event_count, 4);
    assert!(inspections[2].incomplete_attempts.is_empty());
    assert_eq!(
        inspections[0]
            .ordered_audit_events
            .iter()
            .filter(|event| event.event.phase == AuditPhase::OutboundExchangeCompleted)
            .count(),
        2
    );
    assert!(inspections[1].ordered_audit_events.iter().any(|event| {
        event.event.phase == AuditPhase::InboundReplyWriteObserved && event.event.replayed
    }));
}
