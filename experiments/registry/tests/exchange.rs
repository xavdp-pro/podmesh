use podmesh_registry_lab::{
    exchange::{import, Refusal, Snapshot, MAX_WIRE_BYTES},
    Observation, Store,
};
const M: &str = "00000000-0000-4000-8000-000000000001";
const R: &str = "00000000-0000-4000-8000-000000000002";
const E: &str = "00000000-0000-4000-8000-000000000003";
fn store(enrolled: bool) -> Store {
    let s = Store::open(std::path::Path::new(":memory:"), M).unwrap();
    if enrolled {
        s.enroll(E, R, "host:a:").unwrap();
    }
    s
}
fn populate(s: &Store, count: usize) {
    populate_padded(s, count, 0);
}
fn populate_padded(s: &Store, count: usize, padded: usize) {
    let mut prev = None;
    for n in 1..=count {
        let mut b = serde_json::to_vec_pretty(&Observation {
            format: "podmesh-registry-observation-lab/1".into(),
            mesh_uuid: M.into(),
            origin_replica_uuid: R.into(),
            producer_epoch_uuid: E.into(),
            sequence: n as u64,
            previous_event_id: prev,
            dependencies: vec![],
            event_type: "observation.reported".into(),
            resource_key: "host:a:évidence".into(),
            observed_at: "2026-09-12".into(),
            evidence_digest: "a".repeat(64),
            reported_state: "unknown".into(),
        })
        .unwrap();
        if padded > b.len() {
            b.resize(padded, b' ');
        }
        let id = podmesh_registry_lab::digest(&b);
        s.ingest(&id, &b).unwrap();
        prev = Some(id);
    }
}
#[test]
fn bounded_pages_lost_receipt_and_third_store_catchup() {
    let a = store(true);
    let b = store(true);
    let c = store(true);
    populate(&a, 70);
    let snapshot = Snapshot::capture(&a).unwrap();
    let (first, next) = snapshot.page(None).unwrap();
    assert!(next.is_some());
    let first_receipt = import(&b, &first).unwrap();
    assert_eq!(first_receipt.events.len(), 64);
    assert_eq!(
        first_receipt.batch_digest,
        podmesh_registry_lab::digest(&first)
    );
    assert!(first_receipt.events.iter().all(|e| e.stored));
    assert_eq!(first_receipt.advance(&first).unwrap(), next);
    // Simulated lost receipt: exact retransmission changes neither bytes nor count.
    assert!(import(&b, &first).unwrap().events.iter().all(|e| e.stored));
    assert_eq!(b.export().unwrap().len(), 64);
    let (last, end) = snapshot.page(next.as_ref()).unwrap();
    assert!(end.is_none());
    assert!(import(&b, &last).unwrap().advance(&last).unwrap().is_none());
    assert_eq!(a.export().unwrap(), b.export().unwrap());
    assert!(b.states().unwrap().values().all(|s| s == "admitted"));
    // Third replica was offline throughout the initial transfer.
    let relay = Snapshot::capture(&b).unwrap();
    let mut cursor = None;
    loop {
        let (wire, next) = relay.page(cursor.as_ref()).unwrap();
        let receipt = import(&c, &wire).unwrap();
        cursor = receipt.advance(&wire).unwrap();
        assert_eq!(cursor, next);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(a.export().unwrap(), c.export().unwrap());
    assert!(c.states().unwrap().values().all(|s| s == "admitted"));
}
#[test]
fn refuses_unenrolled_writers_and_invalid_envelopes_without_writes() {
    let a = store(true);
    let b = store(false);
    populate(&a, 1);
    let (wire, _) = Snapshot::capture(&a).unwrap().page(None).unwrap();
    let receipt = import(&b, &wire).unwrap();
    assert!(!receipt.events[0].stored);
    assert!(receipt.advance(&wire).is_err());
    assert_eq!(receipt.events[0].refusal, Some(Refusal::Enrollment));
    assert!(b.export().unwrap().is_empty());
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["mesh"] = serde_json::json!("00000000-0000-4000-8000-000000000099");
    assert!(import(&b, &serde_json::to_vec(&value).unwrap()).is_err());
    assert!(import(&b, &vec![b' '; MAX_WIRE_BYTES + 1]).is_err());
    assert!(import(&b, &wire[..wire.len() - 1]).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["records"][0]["hex"] = serde_json::json!("xx");
    assert!(import(&b, &serde_json::to_vec(&value).unwrap()).is_err());
    assert!(b.export().unwrap().is_empty());
}
#[test]
fn cursor_is_bound_to_frozen_snapshot_and_altered_body_is_refused() {
    let a = store(true);
    let b = store(true);
    populate(&a, 70);
    let snapshot = Snapshot::capture(&a).unwrap();
    let (wire, cursor) = snapshot.page(None).unwrap();
    let empty = Snapshot::capture(&store(true)).unwrap();
    assert!(empty.page(cursor.as_ref()).is_err());
    let mut invalid = cursor.unwrap();
    invalid.offset = usize::MAX;
    assert!(snapshot.page(Some(&invalid)).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["records"][0]["hex"] = serde_json::json!("7b7d");
    let altered = serde_json::to_vec(&value).unwrap();
    let receipt = import(&b, &altered).unwrap();
    assert!(!receipt.events[0].stored);
    assert_eq!(receipt.events[0].refusal, Some(Refusal::Validation));
    assert!(receipt.advance(&altered).is_err());
    assert_eq!(b.export().unwrap().len(), 63);
}

#[test]
fn framing_bounds_and_duplicate_fields_refuse_before_storage() {
    let a = store(true);
    let b = store(true);
    populate(&a, 1);
    let (wire, _) = Snapshot::capture(&a).unwrap().page(None).unwrap();
    let base: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    let mut too_many = base.clone();
    too_many["records"] = serde_json::json!(vec![base["records"][0].clone(); 65]);
    assert!(import(&b, &serde_json::to_vec(&too_many).unwrap()).is_err());
    let mut too_large = base.clone();
    too_large["records"] = serde_json::json!((0..5)
        .map(|n| serde_json::json!({"id": format!("{n:064x}"), "hex": "20".repeat(65_536)}))
        .collect::<Vec<_>>());
    assert!(import(&b, &serde_json::to_vec(&too_large).unwrap()).is_err());
    let mut wrong_next = base.clone();
    wrong_next["next"] = serde_json::json!({"snapshot": base["cursor"]["snapshot"], "offset": 9});
    assert!(import(&b, &serde_json::to_vec(&wrong_next).unwrap()).is_err());
    let mut truncated = base.clone();
    truncated["total"] = serde_json::json!(2);
    truncated["next"] = serde_json::Value::Null;
    assert!(import(&b, &serde_json::to_vec(&truncated).unwrap()).is_err());
    let duplicated =
        String::from_utf8(wire)
            .unwrap()
            .replacen("{", "{\"format\":\"duplicate\",", 1);
    assert!(import(&b, duplicated.as_bytes()).is_err());
    assert!(b.export().unwrap().is_empty());
}

#[test]
fn full_size_events_split_at_body_budget_and_snapshot_stays_frozen() {
    let a = store(true);
    populate_padded(&a, 5, 65_536);
    let snapshot = Snapshot::capture(&a).unwrap();
    let (first, next) = snapshot.page(None).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(value["records"].as_array().unwrap().len(), 4);
    assert_eq!(next.as_ref().unwrap().offset, 4);
    assert!(first.len() <= MAX_WIRE_BYTES);
    let (last, end) = snapshot.page(next.as_ref()).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&last).unwrap()["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(end.is_none());
    // New immutable events can coexist in a forked stream; snapshot bytes stay fixed.
    populate(&a, 1);
    assert_eq!(snapshot.page(None).unwrap().0, first);
    assert!(Snapshot::capture(&a).unwrap().page(next.as_ref()).is_err());
    let empty = Snapshot::capture(&store(true)).unwrap();
    assert!(empty.page(None).unwrap().1.is_none());
}
