//! Nested-lab inner Podman metadata reconciliation — log parsing contract from the vzcriu kit counter.
use podmesh::migration_shape_blockers_for_profile;
use serde_json::json;

#[test]
fn migration_nested_outer_shape_unchanged_for_rule11_fixture() {
    let container = json!({
        "HostConfig": {"NetworkMode": "none", "Privileged": true},
        "Mounts": [],
        "Config": {"Tty": false}
    });
    assert!(migration_shape_blockers_for_profile(&container, "nested")
        .unwrap()
        .is_empty());
}
