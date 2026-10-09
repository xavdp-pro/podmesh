//! Destination nested restore hooks assess — manifest/sidecar contract from nested source checkpoint.
use podmesh::migration_shape_blockers_for_profile;
use serde_json::json;

#[test]
fn migration_nested_destination_hooks_assess_is_exported_via_manifest_format() {
    let container = json!({
        "HostConfig": {"NetworkMode": "none", "Privileged": true},
        "Mounts": [],
        "Config": {"Tty": false}
    });
    assert!(migration_shape_blockers_for_profile(&container, "nested")
        .unwrap()
        .is_empty());
}
