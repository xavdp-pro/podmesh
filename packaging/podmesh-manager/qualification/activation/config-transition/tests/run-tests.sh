#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

bash -n "$root/transition-host.sh"
python3 -m py_compile "$root/config-tool.py"
python3 -m py_compile "$root/compare-three-hosts.py"

cat > "$work/map.json" <<'EOF'
{"schema_version":"podmesh-manager-alias-replica-map/v1","aliases":{"lab-a":"replica-a","lab-b":"replica-b","lab-c":"replica-c"}}
EOF
cat > "$work/config.json" <<'EOF'
{"network":{"replica_id":"replica-a","database_path":"/var/lib/podmesh-manager/manager.sqlite","manager":{"logical_manager_id":"logical","replicas":[{"replica_id":"replica-a","host_id":"host-a"},{"replica_id":"replica-b","host_id":"host-b"},{"replica_id":"replica-c","host_id":"host-c"}],"grants":[]},"bind":"127.0.0.1:9443","peers":[{"replica_id":"replica-b","endpoint":"192.0.2.2:9443","shared_key_hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"replica_id":"replica-c","endpoint":"192.0.2.3:9443","shared_key_hex":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]},"control_socket":"/run/podmesh-manager/control.sock","observation_writer_uid":0,"interval_ms":1000,"max_backoff_ms":30000,"incoming_workers":1}
EOF

python3 "$root/config-tool.py" --mode prepare --original "$work/config.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/candidate.json"
python3 "$root/config-tool.py" --mode verify --original "$work/config.json" --candidate "$work/candidate.json" --mapping "$work/map.json" --host-alias lab-a > "$work/shape.json"
jq -e '.grant_count==3 and .replica_count==3 and .all_other_values_preserved and .local_replica_matches_mapping and .grant_scopes==["g2/lab-a/observations","g2/lab-b/observations","g2/lab-c/observations"]' "$work/shape.json" >/dev/null
jq -e 'del(.network.manager.grants) == (input|del(.network.manager.grants))' "$work/config.json" "$work/candidate.json" >/dev/null
jq -e '.network.manager.grants==[{"scope":"g2/lab-a/observations","owner_replica_id":"replica-a"},{"scope":"g2/lab-b/observations","owner_replica_id":"replica-b"},{"scope":"g2/lab-c/observations","owner_replica_id":"replica-c"}]' "$work/candidate.json" >/dev/null

expect_refusal() { if "$@" >/dev/null 2>&1; then echo "accepted invalid case: $*" >&2; exit 1; fi; }
jq '.network.manager.grants=[{"scope":"old","owner_replica_id":"replica-a"}]' "$work/config.json" > "$work/bad.json"
expect_refusal python3 "$root/config-tool.py" --mode prepare --original "$work/bad.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-existing.json"
jq '.aliases["lab-c"]="replica-b"' "$work/map.json" > "$work/bad-map.json"
expect_refusal python3 "$root/config-tool.py" --mode prepare --original "$work/config.json" --mapping "$work/bad-map.json" --host-alias lab-a --output "$work/out-duplicate.json"
jq '.aliases["lab-c"]="replica-z"' "$work/map.json" > "$work/bad-map-set.json"
expect_refusal python3 "$root/config-tool.py" --mode prepare --original "$work/config.json" --mapping "$work/bad-map-set.json" --host-alias lab-a --output "$work/out-set.json"
expect_refusal python3 "$root/config-tool.py" --mode prepare --original "$work/config.json" --mapping "$work/map.json" --host-alias lab-b --output "$work/out-wrong-local.json"
jq '.network.bind="127.0.0.1:9555"' "$work/candidate.json" > "$work/drift.json"
expect_refusal python3 "$root/config-tool.py" --mode verify --original "$work/config.json" --candidate "$work/drift.json" --mapping "$work/map.json" --host-alias lab-a
jq '.network.manager.grants |= reverse' "$work/candidate.json" > "$work/reordered.json"
expect_refusal python3 "$root/config-tool.py" --mode verify --original "$work/config.json" --candidate "$work/reordered.json" --mapping "$work/map.json" --host-alias lab-a
printf '%s' '{"schema_version":"podmesh-manager-alias-replica-map/v1","aliases":{"lab-a":"a","lab-a":"b","lab-b":"b","lab-c":"c"}}' > "$work/duplicate-key-map.json"
expect_refusal python3 "$root/config-tool.py" --mode prepare --original "$work/config.json" --mapping "$work/duplicate-key-map.json" --host-alias lab-a --output "$work/out-key.json"

python3 - "$work" <<'PY'
import hashlib, json, pathlib, sys
root=pathlib.Path(sys.argv[1])
def c(value): return "sha256:"+hashlib.sha256(value.encode()).hexdigest()
grants=[{"scope":f"g2/{alias}/observations","owner_replica_commitment":c("owner-"+alias)} for alias in ("lab-a","lab-b","lab-c")]
for alias in ("lab-a","lab-b","lab-c"):
    value={"schema_version":"podmesh-manager-config-transition-evidence/v1","result":"PASS","action":"applied","host_alias":alias,"candidate":{"package":"podmesh-manager","version":"0.1.0~manager2","binary_sha256":hashlib.sha256(b"binary").hexdigest(),"report_commitment":c("report"),"dpkg_verify":"clean"},"transition":{"source_config_commitment":c("source-"+alias),"applied_config_commitment":c("applied-"+alias),"protected_backup_commitment":c("backup-"+alias),"mapping_commitment":c("mapping"),"activation_markers_commitment":c("markers-"+alias),"grant_count":3,"grants":grants,"all_other_values_preserved":True,"local_replica_matches_mapping":True},"preconditions":{"manager_disabled":True,"manager_inactive":True,"no_manager_process":True,"no_control_socket":True,"no_configured_port_listener":True,"state_directory_empty":True},"offline_validation":{"candidate_valid":True,"final_path_valid":True,"durable_store_checked":False,"network_started":False},"rollback_permitted_only_while":{"state_directory_empty":True,"activation_markers_unchanged":True},"private_values":"absent","claims_not_made":["activation","availability","convergence","DNS","fencing","high availability","replication","takeover"]}
    path=root/f"{alias}-evidence.json"
    path.write_text(json.dumps(value),encoding="utf-8")
    (root/f"{path.name}.sha256").write_text(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n",encoding="ascii")
PY
sidecar() { sha256sum -- "$1" > "$1.sha256"; }
python3 "$root/compare-three-hosts.py" "$work/lab-a-evidence.json" "$work/lab-b-evidence.json" "$work/lab-c-evidence.json" > "$work/comparison.json"
jq -e '.result=="PASS" and .host_count==3 and .grant_count==3 and .same_private_mapping_committed and .same_scope_owner_bindings_committed and .same_qualified_candidate and .private_values=="absent"' "$work/comparison.json" >/dev/null
jq '.transition.grants[0].owner_replica_commitment="sha256:0000000000000000000000000000000000000000000000000000000000000000"' "$work/lab-c-evidence.json" > "$work/bad-evidence.json"
sidecar "$work/bad-evidence.json"
expect_refusal python3 "$root/compare-three-hosts.py" "$work/lab-a-evidence.json" "$work/lab-b-evidence.json" "$work/bad-evidence.json"
jq '.preconditions.state_directory_empty=false' "$work/lab-c-evidence.json" > "$work/bad-evidence.json"
sidecar "$work/bad-evidence.json"
expect_refusal python3 "$root/compare-three-hosts.py" "$work/lab-a-evidence.json" "$work/lab-b-evidence.json" "$work/bad-evidence.json"
jq '.raw_replica_id="private-value"' "$work/lab-c-evidence.json" > "$work/bad-evidence.json"
sidecar "$work/bad-evidence.json"
expect_refusal python3 "$root/compare-three-hosts.py" "$work/lab-a-evidence.json" "$work/lab-b-evidence.json" "$work/bad-evidence.json"
jq '.transition.grants[0].shared_key_hex="private-value"' "$work/lab-c-evidence.json" > "$work/bad-evidence.json"
sidecar "$work/bad-evidence.json"
expect_refusal python3 "$root/compare-three-hosts.py" "$work/lab-a-evidence.json" "$work/lab-b-evidence.json" "$work/bad-evidence.json"

rg -q 'State directory is not empty' "$root/transition-host.sh"
rg -q 'activation markers changed' "$root/transition-host.sh"
rg -q '/run/podmesh-manager-qualification' "$root/transition-host.sh"
rg -q '/etc/podmesh-manager/.manager2-activation-started' "$root/transition-host.sh"
rg -q -- '--validate-config' "$root/transition-host.sh"
rg -q 'mv -fT' "$root/transition-host.sh"
if rg -n 'systemctl (start|restart|enable)|--inspect-store|PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers' "$root/transition-host.sh"; then
  echo 'transition script contains activation or durable-store operation' >&2; exit 1
fi

echo 'PASS: exact additive transition, mapping bindings, preservation and refusal boundaries.'
