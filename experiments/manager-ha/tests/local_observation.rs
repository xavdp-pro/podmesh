//! `Store::highest_local_observation` tells the facts a store appended itself
//! from the facts of its own origin it imported back from a peer: the resident
//! lets only a store whose latest own fact it appended itself use its catch-up
//! window.
use podmesh_manager_ha_lab::{
    durable::{Configuration, Request, Response, Snapshot, Store},
    ReplicaConfig, ScopeGrant,
};
use std::path::Path;

fn configuration() -> Configuration {
    Configuration {
        logical_manager_id: "local-observation".into(),
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
    }
}

fn open(path: &Path, replica: &str) -> Store {
    Store::open(path, configuration(), replica).unwrap()
}

fn observe(store: &mut Store, operation: &str) {
    store
        .execute(&Request::Observe {
            operation_id: operation.into(),
            scope: "scope1".into(),
            subject: "boot".into(),
            exclusive_resource: None,
            active_claim: false,
            value: format!("{operation} value"),
        })
        .unwrap();
}

fn snapshot(store: &mut Store) -> Snapshot {
    match store.execute(&Request::Export {}).unwrap() {
        Response::Snapshot { snapshot } => snapshot,
        _ => unreachable!(),
    }
}

fn import(store: &mut Store, operation: &str, from: Snapshot) {
    store
        .execute(&Request::Import {
            operation_id: operation.into(),
            snapshot: from,
        })
        .unwrap();
}

#[test]
fn only_facts_a_store_appended_itself_count_as_local_observations() {
    let directory = tempfile::tempdir().unwrap();
    let mut original = open(&directory.path().join("r1.sqlite"), "r1");
    assert_eq!(original.highest_local_observation().unwrap(), None);
    for n in 1..=3 {
        observe(&mut original, &format!("boot-{n}"));
    }
    assert_eq!(original.highest_local_observation().unwrap(), Some(3));

    // A peer holds the three facts, and a new store of the same replica imports
    // them back from it: they are of its own origin, not its own appends.
    let mut peer = open(&directory.path().join("r2.sqlite"), "r2");
    import(&mut peer, "from-r1", snapshot(&mut original));
    let mut rebuilt = open(&directory.path().join("r1-rebuilt.sqlite"), "r1");
    import(&mut rebuilt, "from-r2", snapshot(&mut peer));
    assert_eq!(snapshot(&mut rebuilt).facts.len(), 3);
    assert_eq!(rebuilt.highest_local_observation().unwrap(), None);

    // Its next append takes the next sequence, and is its own.
    observe(&mut rebuilt, "boot-4");
    assert_eq!(rebuilt.highest_local_observation().unwrap(), Some(4));
    // The peer appended nothing of the first replica's origin.
    assert_eq!(peer.highest_local_observation().unwrap(), None);
}
