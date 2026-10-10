//! Startup preconditions for the bounded durable lifecycle candidate; no host effects.
use podmesh::{
    store::{migrations, DurableStore, SqliteStore, Value as Stored},
    NodeStore,
};
use serde_json::json;

fn journal() -> SqliteStore {
    let mut store = SqliteStore::open_in_memory().unwrap();
    migrations::apply(&mut store).unwrap();
    store
}

#[test]
fn clean_lifecycle_history_passes_without_mutating_the_store() {
    let mut store = journal();
    store
        .execute(
            "INSERT INTO operations VALUES(?, ?, 'verified', NULL)",
            &[
                Stored::from("isolated-create"),
                Stored::from(
                    json!({"operation":"create", "network_profile":"isolated"}).to_string(),
                ),
            ],
        )
        .unwrap();
    let mut node = NodeStore::Durable(Box::new(store));
    node.validate_startup_scope().unwrap();
    let NodeStore::Durable(mut store) = node else {
        panic!("durable store")
    };
    assert_eq!(
        store.query("SELECT id FROM operations", &[]).unwrap().len(),
        1
    );
}

#[test]
fn effective_interrupted_and_historical_network_effects_all_refuse() {
    for state in ["effective", "applying", "removing", "removed"] {
        let mut store = journal();
        store.execute("INSERT INTO network_effects(kind, `key`, owner, intent, state, operation_id, changed_at) VALUES('bridge', 'fixture', 'owner', '{}', ?, 'effect', 0)", &[Stored::from(state)]).unwrap();
        let mut node = NodeStore::Durable(Box::new(store));
        let error = node.validate_startup_scope().unwrap_err().to_string();
        assert!(error.contains("network_effects"), "{error}");
        let NodeStore::Durable(mut store) = node else {
            panic!("durable store")
        };
        assert_eq!(
            store
                .query_one("SELECT state FROM network_effects", &[])
                .unwrap()
                .unwrap()
                .text(0)
                .unwrap(),
            state
        );
    }
}

#[test]
fn publisher_transition_refuses_before_serving() {
    let mut store = journal();
    store
        .execute(
            "INSERT INTO publisher_transitions VALUES('resource', 'starting', 1, 'effect', 0)",
            &[],
        )
        .unwrap();
    let error = NodeStore::Durable(Box::new(store))
        .validate_startup_scope()
        .unwrap_err()
        .to_string();
    assert!(error.contains("publisher_transitions"), "{error}");
}

#[test]
fn history_without_effect_rows_still_refuses() {
    for request in [
        json!({"operation":"network_route_publish"}),
        json!({"operation":"publisher_startup_withdrawal"}),
        json!({"operation":"create", "network_profile":"managed"}),
    ] {
        let mut store = journal();
        store
            .execute(
                "INSERT INTO operations VALUES('effect', ?, 'pending', NULL)",
                &[Stored::from(request.to_string())],
            )
            .unwrap();
        assert!(NodeStore::Durable(Box::new(store))
            .validate_startup_scope()
            .is_err());
    }
}

#[test]
fn invalid_history_and_unreadable_schema_fail_closed() {
    let mut store = journal();
    store
        .execute(
            "INSERT INTO operations VALUES('effect', 'invalid-json', 'pending', NULL)",
            &[],
        )
        .unwrap();
    assert!(NodeStore::Durable(Box::new(store))
        .validate_startup_scope()
        .is_err());
    let mut store = journal();
    store.execute_batch("DROP TABLE network_routes").unwrap();
    assert!(NodeStore::Durable(Box::new(store))
        .validate_startup_scope()
        .is_err());
}

#[test]
fn sqlite_keeps_its_existing_reconciliation_path() {
    let store = journal();
    store
        .connection()
        .execute(
            "INSERT INTO publisher_transitions VALUES('resource', 'starting', 1, 'effect', 0)",
            [],
        )
        .unwrap();
    NodeStore::Sqlite(store.into_connection())
        .validate_startup_scope()
        .unwrap();
}

#[cfg(feature = "mariadb")]
#[test]
fn real_mariadb_startup_refuses_effect_state_when_a_server_is_named() {
    use podmesh::store::{MariadbConfig, MariadbStore};
    let Some(config) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no isolated test server");
        return;
    };
    let mut store = MariadbStore::open(&config).expect("named MariaDB test server opens");
    migrations::apply(&mut store).unwrap();
    let mut node = NodeStore::Durable(Box::new(store));
    node.validate_startup_scope()
        .expect("test needs a fresh isolated node journal");
    let NodeStore::Durable(store) = &mut node else {
        panic!("durable store")
    };
    store.execute("INSERT INTO publisher_transitions VALUES('startup-guard-fixture', 'starting', 1, 'startup-guard-fixture', 0)", &[]).unwrap();
    let refusal = node.validate_startup_scope().unwrap_err().to_string();
    let NodeStore::Durable(store) = &mut node else {
        panic!("durable store")
    };
    store
        .execute(
            "DELETE FROM publisher_transitions WHERE resource = 'startup-guard-fixture'",
            &[],
        )
        .unwrap();
    assert!(refusal.contains("publisher_transitions"), "{refusal}");
    node.validate_startup_scope().unwrap();
}
