#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

bash -n "$root/capture-host.sh"
bash -n "$root/activate-host.sh"
rg -q 'host_alias.*package_version.*binary_sha256' "$root/activate-host.sh"
rg -q 'Prepared ledger found an unowned temporary drop-in' "$root/activate-host.sh"
rg -q 'ledger_sum.recovered' "$root/activate-host.sh"
rg -q 'write_ledger start-failed' "$root/activate-host.sh"
rg -q 'RECOVERED_NOT_QUALIFIED' "$root/activate-host.sh"
rg -q 'cleanup_restart:true' "$root/activate-host.sh"
python3 -m py_compile "$root/compare-evidence.py" "$root/validate-dropin.py" "$root/graceful-shutdown.py" "$root/wait-ready.py" "$root/append-observation.py"
python3 "$root/tests/test_helpers.py" -q
rg -q '/run/podmesh-manager-qualification' "$root/activate-host.sh"
rg -q '/etc/podmesh-manager/.manager2-activation-started' "$root/activate-host.sh"
marker_line=$(rg -n '^  mark_activation_started$' "$root/activate-host.sh" | head -1 | cut -d: -f1)
start_line=$(rg -n '^  if ! systemctl start podmesh-manager.service' "$root/activate-host.sh" | head -1 | cut -d: -f1)
[ "$marker_line" -lt "$start_line" ] || { echo 'activation marker is not written before systemctl start' >&2; exit 1; }

cat > "$work/good.conf" <<'EOF'
[Service]
Environment=PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers
RestrictAddressFamilies=AF_UNIX AF_INET
IPAddressAllow=192.0.2.1/32
IPAddressAllow=192.0.2.2/32
EOF
python3 "$root/validate-dropin.py" --dropin "$work/good.conf" > "$work/dropin.json"
jq -e '.network_mode=="authenticated-static-peers" and .address_families==["AF_UNIX","AF_INET"] and .peer_allow_count==2 and .peer_allow_prefix_length==32' "$work/dropin.json" >/dev/null
printf '%s\n' '[Service]' 'Environment=PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers' 'RestrictAddressFamilies=AF_UNIX AF_INET' 'IPAddressAllow=192.0.2.1/24' 'IPAddressAllow=192.0.2.2/32' > "$work/bad.conf"
if python3 "$root/validate-dropin.py" --dropin "$work/bad.conf" --quiet; then echo 'accepted non-/32 drop-in' >&2; exit 1; fi

python3 - "$work" <<'PY'
import hashlib,json,pathlib,sys
r=pathlib.Path(sys.argv[1])
def c(x): return "sha256:"+hashlib.sha256(x.encode()).hexdigest()
def h(x): return hashlib.sha256(x.encode()).hexdigest()
aliases=["lab-a","lab-b","lab-c"]; replicas=[c("replica-"+x) for x in aliases]; hosts=[c("host-"+x) for x in aliases]
keys={(0,1):c("key-ab"),(0,2):c("key-ac"),(1,2):c("key-bc")}
shutdown={"schema_version":"podmesh-manager-graceful-shutdown/v1","typed_request_acknowledged":True,"process_exited_successfully":True,"service_inactive":True,"control_socket_absent":True,"forced_signal_used":False}
def evidence(i,stage):
    running=stage in ("active-baseline","converged"); cleanup=stage=="post-cleanup"
    peers=[{"replica_id_commitment":replicas[j],"endpoint_commitment":c(f"endpoint-{j}"),"shared_key_commitment":keys[tuple(sorted((i,j)))]} for j in range(3) if j!=i]
    limits={"network_mode":"authenticated-static-peers","address_families":["AF_UNIX","AF_INET"],"peer_allow_count":2,"peer_allow_prefix_length":32} if running else {"network_mode":None,"address_families":[],"peer_allow_count":0,"peer_allow_prefix_length":None}
    inspection=None
    if stage=="active-baseline": inspection={"schema_version":3,"logical_manager_commitment":c("logical"),"replica_commitment":replicas[i],"logical_history_sha256":h(f"baseline-{i}"),"sqlite_integrity_result":"ok","history_count":0,"receipt_count":0,"audit_event_count":0,"incomplete_attempt_count":0}
    if stage in ("converged","post-cleanup"): inspection={"schema_version":3,"logical_manager_commitment":c("logical"),"replica_commitment":replicas[i],"logical_history_sha256":h("history"),"sqlite_integrity_result":"ok","history_count":3,"receipt_count":3,"audit_event_count":9,"incomplete_attempt_count":0}
    return {"schema_version":"podmesh-manager-live-activation-evidence/v2","host_alias":aliases[i],"stage":stage,
      "package":{"name":"podmesh-manager","version":"0.1.0~manager2","binary_sha256":h("binary"),"dpkg_verify":"clean"},
      "configuration":{"document_commitment":c(f"config-{i}"),"logical_manager_commitment":c("logical"),"local_replica_commitment":replicas[i],"local_host_commitment":hosts[i],"topology_commitment":c("topology"),"peer_count":2,"peers":peers},
      "dropin":{"present":running,"sha256":h("dropin") if running else None,"semantic_limits":limits,"packaged_fragment_sha256":h("fragment"),"inherited_deny_all":True,"effective_policy_configured":running,"effective_policy_commitment":c("effective-policy") if running else None},
      "service":{"load_state":"loaded","active_state":"active" if running else "inactive","sub_state":"running" if running else "dead","unit_file_state":"disabled","main_pid":101+i if running else 0,"invocation_commitment":c(f"invocation-{i}") if running or cleanup else None,"n_restarts":0,"result":"success" if cleanup else "success" if running else "","exec_main_code":"0" if cleanup else "" if not running else "exited","exec_main_status":0},
      "manager_process":{"count":1 if running else 0,"pid":101+i if running else None,"uid":995 if running else None,"argv_commitment":c(f"argv-{i}") if running else None,"argv_count":7 if running else 0},
      "paths":{"state":{"present":True,"uid":995,"gid":995,"mode":"750","content_commitment":c(f"state-{i}-{stage}")},"runtime":{"present":running,"uid":995 if running else None,"gid":995 if running else None,"mode":"700" if running else None,"content_commitment":c(f"runtime-{i}") if running else None},"control_socket":{"present":running,"uid":995 if running else None,"gid":995 if running else None,"mode":"600" if running else None}},
      "listeners":{"status":"available-successful","endpoint_commitment":c(f"endpoint-{i}"),"tcp_listener_count":1 if running else 0,"udp_listener_count":0},
      "stability":{"existing_services_commitment":c("existing"),"podman_containers_commitment":c("containers"),"routes":{"status":"available-successful","commitment":c("routes")},"firewall":{"status":"available-successful","commitment":c("firewall")}},
      "inspection":inspection,"graceful_shutdown":shutdown if cleanup else None}
for i in range(3):
    for stage in ("pre-activation","active-baseline","converged","post-cleanup"):
        path=r/f"{aliases[i]}-{stage}.json"
        path.write_text(json.dumps(evidence(i,stage)))
        (r/f"{path.name}.sha256").write_text(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n")
PY

sidecar() { sha256sum -- "$1" > "$1.sha256"; }

host_args=(--phase host --pre "$work/lab-a-pre-activation.json" --active-baseline "$work/lab-a-active-baseline.json" --converged "$work/lab-a-converged.json" --cleanup "$work/lab-a-post-cleanup.json")
three_args=(--phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-converged.json --cleanup "$work"/*-post-cleanup.json)
"$root/compare-evidence.py" "${host_args[@]}" > "$work/host.json"
jq -e '.status=="PASS" and (.canonical_convergence_evidenced|not) and .ha_claim=="absent"' "$work/host.json" >/dev/null
"$root/compare-evidence.py" "${three_args[@]}" > "$work/three.json"
jq -e '.status=="PASS" and .canonical_convergence_evidenced and .ha_claim=="absent" and .schema_version=="podmesh-manager-live-activation-comparison/v2"' "$work/three.json" >/dev/null

reject_host() { local label=$1 filter=$2; jq "$filter" "$work/lab-a-${3:-post-cleanup}.json" > "$work/bad.json"; sidecar "$work/bad.json"; local args=("${host_args[@]}"); case ${3:-post-cleanup} in pre-activation) args[3]="$work/bad.json";; active-baseline) args[5]="$work/bad.json";; converged) args[7]="$work/bad.json";; post-cleanup) args[9]="$work/bad.json";; esac; if "$root/compare-evidence.py" "${args[@]}" >/dev/null; then echo "accepted $label" >&2; exit 1; fi; }
reject_three() { local label=$1 filter=$2 stage=$3; jq "$filter" "$work/lab-a-$stage.json" > "$work/bad.json"; sidecar "$work/bad.json"; local args=("${three_args[@]}"); case $stage in active-baseline) args=(--phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work/bad.json" "$work/lab-b-active-baseline.json" "$work/lab-c-active-baseline.json" --converged "$work"/*-converged.json --cleanup "$work"/*-post-cleanup.json);; converged) args=(--phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work/bad.json" "$work/lab-b-converged.json" "$work/lab-c-converged.json" --cleanup "$work"/*-post-cleanup.json);; esac; if "$root/compare-evidence.py" "${args[@]}" >/dev/null; then echo "accepted $label" >&2; exit 1; fi; }

reject_host 'unknown route observation' '.stability.routes.status="unknown"'
reject_host 'failed service result' '.service.result="signal"'
reject_host 'nonzero service exit status' '.service.exec_main_status=15'
reject_host 'graceful proof outside cleanup' '.graceful_shutdown={"schema_version":"podmesh-manager-graceful-shutdown/v1","typed_request_acknowledged":true,"process_exited_successfully":true,"service_inactive":true,"control_socket_absent":true,"forced_signal_used":false}' active-baseline
reject_host 'inexact graceful proof' '.graceful_shutdown.extra=true'
reject_host 'runtime wrong mode' '.paths.runtime.mode="755"' active-baseline
reject_host 'socket wrong owner' '.paths.control_socket.uid=996' converged
reject_host 'state owner changed' '.paths.state.uid=996' converged
reject_host 'cleanup inspection regression' '.inspection.history_count=2'
reject_host 'cleanup history mutation without count growth' '.inspection.logical_history_sha256="0000000000000000000000000000000000000000000000000000000000000000"'
reject_host 'widened effective policy' '.dropin.semantic_limits.peer_allow_count=3' active-baseline
reject_host 'missing effective policy observation' '.dropin.effective_policy_configured=false' active-baseline
reject_host 'infrastructure mutation during convergence' '.stability.firewall.commitment="sha256:0000000000000000000000000000000000000000000000000000000000000000"' converged
reject_host 'manager restart during convergence' '.service.main_pid=999 | .manager_process.pid=999 | .service.invocation_commitment="sha256:0000000000000000000000000000000000000000000000000000000000000000"' converged
jq '.inspection.logical_history_sha256="0000000000000000000000000000000000000000000000000000000000000000"' "$work/lab-a-active-baseline.json" > "$work/divergent-baseline.json"
sidecar "$work/divergent-baseline.json"
"$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work/divergent-baseline.json" "$work/lab-b-active-baseline.json" "$work/lab-c-active-baseline.json" --converged "$work"/*-converged.json --cleanup "$work"/*-post-cleanup.json > "$work/baseline-divergence.json"
jq -e '.status=="PASS" and .canonical_convergence_evidenced' "$work/baseline-divergence.json" >/dev/null
reject_three 'divergent converged history' '.inspection.logical_history_sha256="0000000000000000000000000000000000000000000000000000000000000000"' converged
reject_three 'too-short converged history' '.inspection.history_count=2' converged
reject_three 'incomplete converged attempt' '.inspection.incomplete_attempt_count=1' converged
reject_three 'non-reciprocal topology' '.configuration.peers[0].shared_key_commitment="sha256:0000000000000000000000000000000000000000000000000000000000000000"' active-baseline
jq '.inspection.logical_history_sha256="0000000000000000000000000000000000000000000000000000000000000000" | .inspection.history_count=4' "$work/lab-a-post-cleanup.json" > "$work/bad-cleanup.json"
sidecar "$work/bad-cleanup.json"
if "$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-converged.json --cleanup "$work/bad-cleanup.json" "$work/lab-b-post-cleanup.json" "$work/lab-c-post-cleanup.json" >/dev/null; then echo 'accepted divergent post-cleanup history' >&2; exit 1; fi

printf '%s\n' 'PASS: four-stage activation evidence, effective policy, ownership, graceful cleanup, infrastructure stability and converged history boundaries.'
