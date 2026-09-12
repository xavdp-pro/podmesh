use podmesh_manager_ha_lab::{
    ConflictKind, Fact, Reconciliation, Replica, ReplicaConfig, ScopeGrant, Topology,
};

fn topology() -> Topology {
    Topology::new(
        "manager-logical-001",
        vec![
            ReplicaConfig {
                replica_id: "replica-001".into(),
                host_id: "host-a".into(),
            },
            ReplicaConfig {
                replica_id: "replica-002".into(),
                host_id: "host-b".into(),
            },
            ReplicaConfig {
                replica_id: "replica-003".into(),
                host_id: "host-c".into(),
            },
        ],
        vec![
            ScopeGrant {
                scope: "host-a/local".into(),
                owner_replica_id: "replica-001".into(),
            },
            ScopeGrant {
                scope: "host-b/local".into(),
                owner_replica_id: "replica-002".into(),
            },
            ScopeGrant {
                scope: "host-c/local".into(),
                owner_replica_id: "replica-003".into(),
            },
        ],
    )
    .unwrap()
}

fn replicas(topology: &Topology) -> (Replica, Replica, Replica) {
    (
        topology.instantiate("replica-001").unwrap(),
        topology.instantiate("replica-002").unwrap(),
        topology.instantiate("replica-003").unwrap(),
    )
}

fn converge(a: &mut Replica, b: &mut Replica, c: &mut Replica) {
    a.exchange_with(b).unwrap();
    b.exchange_with(c).unwrap();
    a.exchange_with(c).unwrap();
}

#[test]
fn topology_requires_one_replica_per_distinct_host_and_one_owner_per_scope() {
    assert!(Topology::new(
        "manager",
        vec![
            ReplicaConfig {
                replica_id: "one".into(),
                host_id: "same".into(),
            },
            ReplicaConfig {
                replica_id: "two".into(),
                host_id: "same".into(),
            },
        ],
        vec![],
    )
    .is_err());
    assert!(Topology::new(
        "manager",
        vec![
            ReplicaConfig {
                replica_id: "same".into(),
                host_id: "host-a".into(),
            },
            ReplicaConfig {
                replica_id: "same".into(),
                host_id: "host-b".into(),
            },
        ],
        vec![],
    )
    .is_err());
    assert!(Topology::new(
        "manager",
        vec![ReplicaConfig {
            replica_id: "one".into(),
            host_id: "host".into(),
        }],
        vec![
            ScopeGrant {
                scope: "scope".into(),
                owner_replica_id: "one".into(),
            },
            ScopeGrant {
                scope: "scope".into(),
                owner_replica_id: "one".into(),
            },
        ],
    )
    .is_err());
    assert!(Topology::new(
        "manager",
        vec![
            ReplicaConfig {
                replica_id: "one".into(),
                host_id: "host-a".into(),
            },
            ReplicaConfig {
                replica_id: "two".into(),
                host_id: "host-b".into(),
            },
        ],
        vec![
            ScopeGrant {
                scope: "host-a".into(),
                owner_replica_id: "one".into(),
            },
            ScopeGrant {
                scope: "host-a/local".into(),
                owner_replica_id: "two".into(),
            },
        ],
    )
    .is_err());
}

#[test]
fn partition_allows_only_independent_owned_scopes_then_reconnects() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);

    a.observe("host-a/local", "universe-a", None, false, "healthy-at-a")
        .unwrap();
    b.observe("host-b/local", "universe-b", None, false, "healthy-at-b")
        .unwrap();
    assert!(a
        .observe(
            "host-b/local",
            "universe-b",
            None,
            false,
            "invented-remote-state"
        )
        .is_err());
    let fabricated = Fact {
        event_id: "replica-001:00000000000000000001".into(),
        logical_manager_id: topology.logical_manager_id().into(),
        origin_replica_id: "replica-001".into(),
        origin_host_id: "host-a".into(),
        producer_sequence: 1,
        scope: "host-b/local".into(),
        subject: "universe-b".into(),
        subject_revision: 1,
        predecessor: None,
        exclusive_resource: None,
        active_claim: false,
        value: "fabricated-import".into(),
    };
    assert!(c.ingest(fabricated).is_err());

    assert!(Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).is_err());
    converge(&mut a, &mut b, &mut c);
    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();

    assert_eq!(reconciled.coordinator_replica_id(), "replica-001");
    assert_eq!(reconciled.view().current.len(), 2);
    assert!(reconciled.view().conflicts.is_empty());
}

#[test]
fn failed_exchange_does_not_partially_mutate_either_replica() {
    let topology = topology();
    let (mut a, mut b, _) = replicas(&topology);
    let original = a
        .observe("host-a/local", "universe-a", None, false, "original")
        .unwrap();
    let conflicting_bytes = Fact {
        value: "different-content-under-same-event-id".into(),
        ..original
    };
    b.ingest(conflicting_bytes).unwrap();
    let before_a: Vec<_> = a.history_ids().into_iter().map(str::to_string).collect();
    let before_b: Vec<_> = b.history_ids().into_iter().map(str::to_string).collect();
    assert!(a.exchange_with(&mut b).is_err());
    assert_eq!(a.history_ids(), before_a);
    assert_eq!(b.history_ids(), before_b);
}

#[test]
fn coordinator_priority_never_overwrites_a_newer_fact() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);
    let first = b
        .observe(
            "host-b/local",
            "universe-b",
            None,
            false,
            "older-observation",
        )
        .unwrap();
    a.ingest(first).unwrap();
    b.observe(
        "host-b/local",
        "universe-b",
        None,
        false,
        "newer-observation",
    )
    .unwrap();

    converge(&mut a, &mut b, &mut c);
    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();
    let fact = reconciled
        .view()
        .current
        .values()
        .find(|fact| fact.subject == "universe-b")
        .unwrap();
    assert_eq!(reconciled.coordinator_replica_id(), "replica-001");
    assert_eq!(fact.subject_revision, 2);
    assert_eq!(fact.value, "newer-observation");
    assert_eq!(fact.origin_replica_id, "replica-002");
}

#[test]
fn exclusive_conflict_retains_both_histories_and_blocks_the_resource() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);
    a.observe(
        "host-a/local",
        "universe-a",
        Some("ip:10.77.1.10"),
        true,
        "claim",
    )
    .unwrap();
    b.observe(
        "host-b/local",
        "universe-b",
        Some("ip:10.77.1.10"),
        true,
        "claim",
    )
    .unwrap();
    converge(&mut a, &mut b, &mut c);

    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();
    let conflict = reconciled
        .view()
        .conflicts
        .iter()
        .find(|conflict| conflict.resource == "ip:10.77.1.10")
        .unwrap();
    assert_eq!(conflict.kind, ConflictKind::ExclusiveResource);
    assert_eq!(conflict.event_ids.len(), 2);
    assert!(reconciled.view().current.is_empty());
    assert_eq!(reconciled.view().blocked_subjects.len(), 2);
    assert_eq!(a.history_len(), 2);
    assert!(reconciled
        .authorize_exclusive_service("replica-001", "ip:10.77.1.10")
        .is_err());
}

#[test]
fn inactive_replicas_never_advertise_the_active_service() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);
    a.observe(
        "host-a/local",
        "manager-instance-a",
        Some("manager:service-ip"),
        true,
        "explicit-active-claim",
    )
    .unwrap();
    converge(&mut a, &mut b, &mut c);
    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();

    assert!(!a.advertises("manager:service-ip", None));
    assert!(!b.advertises("manager:service-ip", None));
    assert!(reconciled
        .authorize_exclusive_service("replica-002", "manager:service-ip")
        .is_err());
    let permit = reconciled
        .authorize_exclusive_service("replica-001", "manager:service-ip")
        .unwrap();
    assert!(a.advertises("manager:service-ip", Some(&permit)));
    assert!(!b.advertises("manager:service-ip", Some(&permit)));
    assert!(!c.advertises("manager:service-ip", Some(&permit)));
    a.observe(
        "host-a/local",
        "later-observation",
        None,
        false,
        "history-changed-after-reconciliation",
    )
    .unwrap();
    assert!(!a.advertises("manager:service-ip", Some(&permit)));
}

#[test]
fn coordinator_cannot_take_an_uncontested_resource_owned_by_another_replica() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);
    b.observe(
        "host-b/local",
        "manager-instance-b",
        Some("manager:service-ip"),
        true,
        "active-on-b",
    )
    .unwrap();
    converge(&mut a, &mut b, &mut c);
    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();
    assert_eq!(reconciled.coordinator_replica_id(), "replica-001");
    assert!(reconciled
        .authorize_exclusive_service("replica-001", "manager:service-ip")
        .is_err());
    assert!(!a.advertises("manager:service-ip", None));
}

#[test]
fn quarantined_claim_keeps_its_exclusive_resource_blocked() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);
    let first = a
        .observe(
            "host-a/local",
            "manager-instance-a",
            Some("manager:service-ip"),
            true,
            "active-on-a",
        )
        .unwrap();
    let orphan = Fact {
        event_id: "replica-001:00000000000000000002".into(),
        producer_sequence: 2,
        subject_revision: 2,
        predecessor: Some("replica-001:00000000000000009999".into()),
        value: "orphan-update".into(),
        ..first
    };
    a.ingest(orphan).unwrap();
    converge(&mut a, &mut b, &mut c);
    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();
    assert!(reconciled
        .view()
        .exclusive_resource_blocked("manager:service-ip"));
    assert!(reconciled
        .authorize_exclusive_service("replica-001", "manager:service-ip")
        .is_err());
}

#[test]
fn stale_copy_catches_up_without_replaying_an_old_head() {
    let topology = topology();
    let (mut a, mut b, mut c) = replicas(&topology);
    a.observe("host-a/local", "universe-a", None, false, "revision-one")
        .unwrap();
    a.exchange_with(&mut c).unwrap();
    a.observe("host-a/local", "universe-a", None, false, "revision-two")
        .unwrap();
    assert_eq!(
        c.materialize().current.values().next().unwrap().value,
        "revision-one"
    );

    converge(&mut a, &mut b, &mut c);
    let reconciled = Reconciliation::after_full_exchange(&topology, &[&a, &b, &c]).unwrap();
    assert_eq!(c.history_len(), 2);
    assert_eq!(
        reconciled.view().current.values().next().unwrap().value,
        "revision-two"
    );
}

#[test]
fn missing_predecessor_and_same_revision_forks_are_blocked() {
    let topology = topology();
    let (mut a, _, _) = replicas(&topology);
    let first = a
        .observe("host-a/local", "universe-a", None, false, "revision-one")
        .unwrap();
    let mut fork: Fact = a
        .observe("host-a/local", "universe-a", None, false, "revision-two")
        .unwrap();
    fork.event_id = "replica-001:00000000000000000003".into();
    fork.producer_sequence = 3;
    fork.value = "competing-revision-two".into();
    a.ingest(fork).unwrap();
    assert!(a
        .materialize()
        .conflicts
        .iter()
        .any(|conflict| conflict.kind == ConflictKind::SubjectFork));
    assert!(a
        .observe(
            "host-a/local",
            "universe-a",
            None,
            false,
            "must-not-extend-a-fork"
        )
        .is_err());

    let mut stale = topology.instantiate("replica-001").unwrap();
    let orphan_fact = Fact {
        event_id: "replica-001:00000000000000000004".into(),
        producer_sequence: 4,
        subject_revision: 2,
        predecessor: Some("replica-001:00000000000000009999".into()),
        value: "orphan".into(),
        ..first
    };
    stale.ingest(orphan_fact).unwrap();
    assert!(stale
        .materialize()
        .conflicts
        .iter()
        .any(|conflict| conflict.kind == ConflictKind::MissingPredecessor));
    assert!(stale
        .observe(
            "host-a/local",
            "universe-a",
            None,
            false,
            "must-not-extend-an-orphan"
        )
        .is_err());
}
