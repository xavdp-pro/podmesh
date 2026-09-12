//! Three independent executable replicas exchange over loopback TCP.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use podmesh_manager_ha_lab::{
    durable::{Configuration, Request, Store},
    ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::{ConfigurationFile, Peer};
use tempfile::TempDir;

fn key_for(left: usize, right: usize) -> String {
    match (left.min(right), left.max(right)) {
        (1, 2) => "11".repeat(32),
        (1, 3) => "22".repeat(32),
        (2, 3) => "33".repeat(32),
        _ => unreachable!(),
    }
}

fn unused_addresses() -> [SocketAddr; 3] {
    let listeners: Vec<_> = (0..3)
        .map(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap())
        .collect();
    let addresses: Vec<_> = listeners
        .iter()
        .map(|listener| listener.local_addr().unwrap())
        .collect();
    drop(listeners);
    addresses.try_into().unwrap()
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

fn configuration(
    directory: &TempDir,
    replica_id: &str,
    addresses: &[SocketAddr],
) -> ConfigurationFile {
    let replica_index: usize = replica_id[1..].parse().unwrap();
    ConfigurationFile {
        replica_id: replica_id.into(),
        database_path: directory.path().join(format!("{replica_id}.sqlite")),
        manager: manager(),
        bind: addresses[replica_index - 1],
        peers: (1..=3)
            .filter(|index| *index != replica_index)
            .map(|index| Peer {
                replica_id: format!("r{index}"),
                endpoint: addresses[index - 1],
                shared_key_hex: key_for(replica_index, index),
            })
            .collect(),
    }
}

#[test]
fn three_replica_processes_catch_up_after_one_was_offline() {
    let directory = tempfile::tempdir().unwrap();
    let addresses = unused_addresses();
    let r1 = configuration(&directory, "r1", &addresses);
    let r2 = configuration(&directory, "r2", &addresses);
    let r3 = configuration(&directory, "r3", &addresses);
    for configuration in [&r1, &r2, &r3] {
        fs::write(
            directory
                .path()
                .join(format!("{}.json", configuration.replica_id)),
            serde_json::to_vec(configuration).unwrap(),
        )
        .unwrap();
    }
    let mut source = Store::open(&r1.database_path, r1.manager.clone(), "r1").unwrap();
    source
        .execute(&Request::Observe {
            operation_id: "origin-observe".into(),
            scope: "scope1".into(),
            subject: "universe".into(),
            exclusive_resource: None,
            active_claim: false,
            value: "survives-partition".into(),
        })
        .unwrap();

    let binary = env!("CARGO_BIN_EXE_podmesh-manager-network-lab");
    let mut replica_two = Command::new(binary)
        .args([
            "serve-once",
            directory.path().join("r2.json").to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut replica_three = Command::new(binary)
        .args([
            "serve-once",
            directory.path().join("r3.json").to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(100));

    // The source is a third executable process, while both destinations listen.
    for (peer, operation, nonce) in [
        ("r2", "catch-up-r2", "nonce-r2"),
        ("r3", "catch-up-r3", "nonce-r3"),
    ] {
        let output = Command::new(binary)
            .args([
                "sync",
                directory.path().join("r1.json").to_str().unwrap(),
                peer,
                operation,
                nonce,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(replica_two.wait().unwrap().success());
    assert!(replica_three.wait().unwrap().success());

    for configuration in [&r2, &r3] {
        let mut store = Store::open(
            &configuration.database_path,
            configuration.manager.clone(),
            &configuration.replica_id,
        )
        .unwrap();
        let inspection = store.execute(&Request::Inspect {}).unwrap();
        assert!(matches!(
            inspection,
            podmesh_manager_ha_lab::durable::Response::Inspection { history_len: 1, .. }
        ));
    }
}
