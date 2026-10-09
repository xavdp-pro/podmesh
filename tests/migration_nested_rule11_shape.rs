//! Rule 11 outer-universe shape for `migration_profile: nested` — fixture from
//! `podmesh-lab/records/slice-b-move-2026-10-07/nested-shape-inspect.txt` (privileged, network none, no mounts).
//! Does not exercise the API or Podman; only the shared assess rules exported for lab probes.
use podmesh::migration_shape_blockers_for_profile;
use serde_json::json;

#[test]
fn slice_b_nested_outer_shape_has_no_migration_shape_blockers() {
    let container = json!({
        "HostConfig": {"NetworkMode": "none", "Privileged": true},
        "Mounts": [],
        "Config": {"Tty": false}
    });
    let nested = migration_shape_blockers_for_profile(&container, "nested").unwrap();
    assert!(nested.is_empty(), "nested profile blockers: {nested:?}");
    let flat = migration_shape_blockers_for_profile(&container, "flat").unwrap();
    assert!(
        flat.iter().any(|b| b.contains("privileged")),
        "flat must still refuse privileged outer: {flat:?}"
    );
}
