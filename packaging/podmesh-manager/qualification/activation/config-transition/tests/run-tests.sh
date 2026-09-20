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

# The second exact transition starts from the three-grant configuration and changes one operational key.
python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/candidate.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/workers.json"
python3 "$root/config-tool.py" --transition incoming-workers --mode verify --original "$work/candidate.json" --candidate "$work/workers.json" --mapping "$work/map.json" --host-alias lab-a > "$work/workers-shape.json"
jq -e '.schema_version=="podmesh-manager-config-shape-verification/v2" and .transition_kind=="incoming-workers" and .changed_keys==["incoming_workers"] and .incoming_workers=={"from":1,"to":2} and .grant_count==3 and .all_other_values_preserved and .local_replica_matches_mapping' "$work/workers-shape.json" >/dev/null
jq -e 'del(.incoming_workers) == (input|del(.incoming_workers))' "$work/candidate.json" "$work/workers.json" >/dev/null
jq -e '.incoming_workers==2' "$work/workers.json" >/dev/null
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/config.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-zero-grants.json"
jq '.incoming_workers=2' "$work/candidate.json" > "$work/already-two.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/already-two.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-already.json"
jq '.incoming_workers=true' "$work/candidate.json" > "$work/bool-workers.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/bool-workers.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-bool.json"
jq '.incoming_workers=3' "$work/workers.json" > "$work/three-workers.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode verify --original "$work/candidate.json" --candidate "$work/three-workers.json" --mapping "$work/map.json" --host-alias lab-a
jq '.interval_ms=500' "$work/workers.json" > "$work/workers-drift.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode verify --original "$work/candidate.json" --candidate "$work/workers-drift.json" --mapping "$work/map.json" --host-alias lab-a
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/candidate.json" --mapping "$work/map.json" --host-alias lab-b --output "$work/out-workers-wrong-local.json"
# Neither transition accepts the other's result.
expect_refusal python3 "$root/config-tool.py" --mode verify --original "$work/candidate.json" --candidate "$work/workers.json" --mapping "$work/map.json" --host-alias lab-a
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode verify --original "$work/config.json" --candidate "$work/candidate.json" --mapping "$work/map.json" --host-alias lab-a
# jq canonicalises the JSON float 1.0 to 1, so the number-shaped variants are written as raw text.
python3 - "$work" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
source = (root / "candidate.json").read_text()
(root / "float-workers.json").write_text(source.replace('"incoming_workers": 1', '"incoming_workers": 1.0'))
(root / "string-workers.json").write_text(source.replace('"incoming_workers": 1', '"incoming_workers": "1"'))
(root / "float-applied.json").write_text((root / "workers.json").read_text().replace('"incoming_workers": 2', '"incoming_workers": 2.0'))
PY
grep -q '"incoming_workers": 1\.0' "$work/float-workers.json" && grep -q '"incoming_workers": 2\.0' "$work/float-applied.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/float-workers.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-float.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/string-workers.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-string.json"
# Python equality alone would accept the float 2.0 as the integer 2; the manager's parser would not.
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode verify --original "$work/candidate.json" --candidate "$work/float-applied.json" --mapping "$work/map.json" --host-alias lab-a
jq 'del(.incoming_workers)' "$work/candidate.json" > "$work/no-workers.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/no-workers.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-none.json"
jq '.network.manager.grants |= reverse' "$work/candidate.json" > "$work/reordered-grants.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/reordered-grants.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-reordered.json"
jq '.network.manager.grants[0].extra="x"' "$work/candidate.json" > "$work/extra-grant-key.json"
expect_refusal python3 "$root/config-tool.py" --transition incoming-workers --mode prepare --original "$work/extra-grant-key.json" --mapping "$work/map.json" --host-alias lab-a --output "$work/out-extra.json"

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

# Evidence of the operational transition: schema v2, one changed key, disclosed durable state and boundary marker.
python3 - "$work" <<'PY'
import hashlib, json, pathlib, sys
root=pathlib.Path(sys.argv[1])
def c(value): return "sha256:"+hashlib.sha256(value.encode()).hexdigest()
grants=[{"scope":f"g2/{alias}/observations","owner_replica_commitment":c("owner-"+alias)} for alias in ("lab-a","lab-b","lab-c")]
for alias in ("lab-a","lab-b","lab-c"):
    value={"schema_version":"podmesh-manager-config-transition-evidence/v2","transition_kind":"incoming-workers","result":"PASS","action":"applied","host_alias":alias,"candidate":{"package":"podmesh-manager","version":"0.1.0~manager2","binary_sha256":hashlib.sha256(b"binary").hexdigest(),"report_commitment":c("report"),"dpkg_verify":"clean"},"transition":{"source_config_commitment":c("source-"+alias),"applied_config_commitment":c("applied-"+alias),"protected_backup_commitment":c("backup-"+alias),"mapping_commitment":c("mapping"),"activation_markers_commitment":c("markers-"+alias),"grant_count":3,"grants":grants,"all_other_values_preserved":True,"local_replica_matches_mapping":True,"changed_keys":["incoming_workers"],"incoming_workers":{"from":1,"to":2},"state_listing_commitment":c("listing")},"preconditions":{"manager_disabled":True,"manager_inactive":True,"no_manager_process":True,"no_control_socket":True,"no_configured_port_listener":True,"durable_state_present":True,"activation_marker_present":alias!="lab-c","state_directory_listing_unchanged":True},"offline_validation":{"candidate_valid":True,"final_path_valid":True,"durable_store_checked":False,"network_started":False},"rollback_permitted_only_while":{"activation_markers_unchanged":True},"private_values":"absent","claims_not_made":["activation","availability","convergence","DNS","fencing","high availability","replication","takeover"]}
    path=root/f"{alias}-workers.json"
    path.write_text(json.dumps(value),encoding="utf-8")
    (root/f"{path.name}.sha256").write_text(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n",encoding="ascii")
PY
python3 "$root/compare-three-hosts.py" --transition incoming-workers "$work/lab-a-workers.json" "$work/lab-b-workers.json" "$work/lab-c-workers.json" > "$work/workers-comparison.json"
jq -e '.result=="PASS" and .schema_version=="podmesh-manager-config-transition-comparison/v2" and .transition_kind=="incoming-workers" and .changed_keys==["incoming_workers"] and .incoming_workers=={"from":1,"to":2} and .grant_count==3 and .all_hosts_inactive_before_transition and (has("all_hosts_inactive_and_empty_before_transition")|not) and .durable_state_present=={"lab-a":true,"lab-b":true,"lab-c":true} and .activation_marker_present=={"lab-a":true,"lab-b":true,"lab-c":false} and .private_values=="absent"' "$work/workers-comparison.json" >/dev/null
# Each comparator kind refuses the other kind's evidence.
expect_refusal python3 "$root/compare-three-hosts.py" "$work/lab-a-workers.json" "$work/lab-b-workers.json" "$work/lab-c-workers.json"
expect_refusal python3 "$root/compare-three-hosts.py" --transition incoming-workers "$work/lab-a-evidence.json" "$work/lab-b-evidence.json" "$work/lab-c-evidence.json"
reject_workers() { jq "$1" "$work/lab-c-workers.json" > "$work/bad-workers.json"; sidecar "$work/bad-workers.json"; expect_refusal python3 "$root/compare-three-hosts.py" --transition incoming-workers "$work/lab-a-workers.json" "$work/lab-b-workers.json" "$work/bad-workers.json"; }
reject_workers '.transition.incoming_workers.to=3'
reject_workers '.transition.changed_keys=["incoming_workers","interval_ms"]'
reject_workers '.preconditions.no_control_socket=false'
reject_workers 'del(.preconditions.durable_state_present)'
reject_workers '.preconditions.durable_state_present="yes"'
reject_workers 'del(.preconditions.state_directory_listing_unchanged)'
reject_workers '.preconditions.state_directory_listing_unchanged=false'
reject_workers 'del(.transition.state_listing_commitment)'
reject_workers '.transition.state_listing_commitment="not-a-commitment"'
reject_workers '.rollback_permitted_only_while={"state_directory_empty":true,"activation_markers_unchanged":true}'
reject_workers '.transition.grants[1].owner_replica_commitment="sha256:0000000000000000000000000000000000000000000000000000000000000000"'

rg -q 'State directory is not empty' "$root/transition-host.sh"
rg -q 'Persistent activation marker does not bind this host and candidate' "$root/transition-host.sh"
rg -q 'ledger transition kind mismatch' "$root/transition-host.sh"
# Only the first-store-open transition may assert an empty state directory in its closure document.
rg -q 'rollback-window-closure/v2' "$root/transition-host.sh"
python3 - "$root/transition-host.sh" <<'PY'
import sys, pathlib
text = pathlib.Path(sys.argv[1]).read_text()
seal = text.split("podmesh-manager-rollback-window-closure/v1", 1)[1].split("\nPY\n", 1)[0]
if 'del document["state_directory_empty_at_closure"]' not in seal:
    raise SystemExit("the closure document keeps its emptiness claim for a transition that does not require one")
if 'document["transition_kind"]=transition' not in seal:
    raise SystemExit("the closure document does not name its transition kind")
PY
rg -q 'activation markers changed' "$root/transition-host.sh"
rg -q '/run/podmesh-manager-qualification' "$root/transition-host.sh"
rg -q '/etc/podmesh-manager/.manager2-activation-started' "$root/transition-host.sh"
rg -q -- '--validate-config' "$root/transition-host.sh"
rg -q 'mv -fT' "$root/transition-host.sh"
if rg -n 'systemctl (start|restart|enable)|--inspect-store|PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers' "$root/transition-host.sh"; then
  echo 'transition script contains activation or durable-store operation' >&2; exit 1
fi

echo 'PASS: exact additive transition, exact operational transition, mapping bindings, preservation and refusal boundaries.'
