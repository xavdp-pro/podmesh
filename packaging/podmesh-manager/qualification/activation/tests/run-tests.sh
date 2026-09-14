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
# The comparator joins replica IDs, the logical manager ID and endpoints across observation sites. A value the
# collector commits under a site-specific label, or a right label over the wrong value, can never join, and the
# synthetic fixtures below cannot notice: they commit every identity under one label by construction. So the
# check is made at each site that must carry the call, inside the shell function that observes it.
python3 - "$root/capture-host.sh" <<'PY'
import re, sys, pathlib
text = pathlib.Path(sys.argv[1]).read_text()
def code(s): return "\n".join(l for l in s.splitlines() if not l.lstrip().startswith("#"))
# One slice per shell function, bounded by its own closing brace at column 0: the last function's slice must
# not run on into the main body, where a literal could satisfy a check meant for one observing site.
bodies = {m.group(1): code(m.group(2)) for m in re.finditer(r'^([a-z_]+)\(\) \{\n(.*?)^\}$', text, re.M | re.S)}
required = {
    "configuration": ['commit_text logical-manager-id ', 'commit_text replica-id "$(jq -r .replica ', 'commit_text replica-id "$replica"', 'commit_text endpoint "$endpoint"', 'commit_text peer-key "$key"'],
    "listeners": ['commit_text endpoint "$endpoint"'],
    "inspection": ['commit_text logical-manager-id ', 'commit_text replica-id "$(jq -r .replica_id '],
}
for fn, calls in required.items():
    if fn not in bodies: raise SystemExit(f"{fn}() is no longer a brace-delimited function of capture-host.sh; the label check cannot see its site")
    for call in calls:
        if call not in bodies[fn]: raise SystemExit(f"{fn}() no longer commits {call!r}; the comparator joins that value across sites")
stale = {"local-replica", "peer-replica", "inspection-replica", "logical-manager", "inspection-logical", "peer-endpoint", "bind-endpoint"} & set(re.findall(r'commit_text (\S+) ', code(text)))
if stale: raise SystemExit(f"site-specific commitment labels break the comparator join: {sorted(stale)}")
PY
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
    limits={"network_mode":"authenticated-static-peers","address_families":["AF_UNIX","AF_INET"],"peer_allow_count":2,"peer_allow_prefix_length":32,"sha256":h("dropin")} if running else {"network_mode":None,"address_families":[],"peer_allow_count":0,"peer_allow_prefix_length":None}
    def inspect(history, hc, rc, ac, attempts, exchanges):
        return {"store_present":True,"schema_version":4,"logical_manager_commitment":c("logical"),
                "replica_commitment":replicas[i],"logical_history_sha256":history,
                "receipt_set_sha256":h(f"receipts-{i}-{stage}"),"audit_set_sha256":h(f"audits-{i}-{stage}"),
                "sqlite_integrity_result":"ok","history_count":hc,"receipt_count":rc,"audit_event_count":ac,
                "incomplete_attempt_count":len(attempts),"incomplete_attempts":attempts,
                "unaudited_import_receipt_count":0,"unaudited_import_receipt_commitments":[],
                "imported_operation_commitments":[]}
    # One folded exchange per nonce: an inbound exchange collapses four audit rows, a
    # completed outbound attempt two. The shapes are the ones measured on a preserved store.
    def served(k):
        return {"nonce_commitment":c(f"nonce-{i}-{k}"),"nonce_authority":"peer-validated","joinable":True,
                "direction":"inbound","phases_reached":["inbound_request_observed","inbound_import_committed",
                "inbound_reply_prepared","inbound_reply_write_observed"],"row_count":4,
                "peer_commitment":replicas[(i+1)%3],"operation_commitment":c(f"op-{i}-{k}"),
                "request_sha256_commitment":c(f"rq-{i}-{k}"),"reply_sha256_commitment":c(f"rp-{i}-{k}"),
                "local_receipt_commitment":c(f"rcpt-{i}-{k}"),"remote_receipt_commitment":None,
                "request_frame_bytes":2735,"reply_frame_bytes":626,"request_announced_body_bytes":2731,
                "reply_announced_body_bytes":622,"outcomes":["accepted"],"replayed":False}
    inspection=None; exchanges=None
    if stage in ("pre-activation","active-baseline"):
        inspection=inspect(h(f"baseline-{i}"),0,0,0,[],None); exchanges=[]
    if stage in ("converged","post-cleanup"):
        exchanges=[served(0),served(1)]
        inspection=inspect(h("history"),3,3,sum(r["row_count"] for r in exchanges),[],None)
    return {"schema_version":"podmesh-manager-live-activation-evidence/v3","host_alias":aliases[i],"stage":stage,
      "package":{"name":"podmesh-manager","version":"0.1.0~manager2","binary_sha256":h("binary"),"dpkg_verify":"clean"},
      "configuration":{"document_commitment":c(f"config-{i}"),"logical_manager_commitment":c("logical"),"local_replica_commitment":replicas[i],"local_host_commitment":hosts[i],"topology_commitment":c("topology"),"peer_count":2,"peers":peers},
      "dropin":{"present":running,"sha256":h("dropin") if running else None,"semantic_limits":limits,"packaged_fragment_sha256":h("fragment"),"inherited_deny_all":True,"effective_policy_configured":running,"effective_policy_commitment":c("effective-policy") if running else None},
      "service":{"load_state":"loaded","active_state":"active" if running else "inactive","sub_state":"running" if running else "dead","unit_file_state":"disabled","main_pid":101+i if running else 0,"invocation_commitment":c(f"invocation-{i}") if running or cleanup else None,"n_restarts":0,"result":"success" if cleanup else "success" if running else "","exec_main_code":"0" if cleanup else "" if not running else "exited","exec_main_status":0},
      "manager_process":{"count":1 if running else 0,"pid":101+i if running else None,"uid":995 if running else None,"argv_commitment":c(f"argv-{i}") if running else None,"argv_count":7 if running else 0},
      "paths":{"state":{"present":True,"uid":995,"gid":995,"mode":"750","content_commitment":c(f"state-{i}-{stage}")},"runtime":{"present":running,"uid":995 if running else None,"gid":995 if running else None,"mode":"700" if running else None,"content_commitment":c(f"runtime-{i}") if running else None},"control_socket":{"present":running,"uid":995 if running else None,"gid":995 if running else None,"mode":"600" if running else None}},
      "listeners":{"status":"available-successful","endpoint_commitment":c(f"endpoint-{i}"),"tcp_listener_count":1 if running else 0,"udp_listener_count":0},
      "stability":{"existing_services_commitment":c("existing"),"podman_containers_commitment":c("containers"),"routes":{"status":"available-successful","commitment":c("routes")},"firewall":{"status":"available-successful","commitment":c("firewall")}},
      "inspection":inspection,"exchanges":exchanges,"graceful_shutdown":shutdown if cleanup else None}
for i in range(3):
    for stage in ("pre-activation","active-baseline","converged","post-cleanup"):
        path=r/f"{aliases[i]}-{stage}.json"
        path.write_text(json.dumps(evidence(i,stage)))
        (r/f"{path.name}.sha256").write_text(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path}\n")
PY

sidecar() { sha256sum -- "$1" > "$1.sha256"; }
# `jq -e` on an EMPTY file exits 0 without ever evaluating its filter. So an assertion made
# against a report the comparator never wrote — because it crashed rather than refusing —
# passes silently, and the suite reports success for a comparator that cannot produce a
# verdict. Every assertion about a refusal report goes through this instead.
report_says() { local path=$1 filter=$2 message=$3; [ -s "$path" ] || { echo "$message: the comparator produced no report at all, which is a crash and not a refusal" >&2; exit 1; }; jq -e "$filter" "$path" >/dev/null || { echo "$message" >&2; exit 1; }; }

host_args=(--phase host --pre "$work/lab-a-pre-activation.json" --active-baseline "$work/lab-a-active-baseline.json" --converged "$work/lab-a-converged.json" --cleanup "$work/lab-a-post-cleanup.json")
three_args=(--phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-converged.json --cleanup "$work"/*-post-cleanup.json)
"$root/compare-evidence.py" "${host_args[@]}" > "$work/host.json"
jq -e '.status=="PASS" and (.canonical_convergence_evidenced|not) and .ha_claim=="absent"' "$work/host.json" >/dev/null
"$root/compare-evidence.py" "${three_args[@]}" > "$work/three.json"
jq -e '.status=="PASS" and .canonical_convergence_evidenced and .ha_claim=="absent" and .schema_version=="podmesh-manager-live-activation-comparison/v3"' "$work/three.json" >/dev/null
# The passing fixtures must exercise the bound hash rather than bypass it: five fields with the inner hash equal to the outer one while active, four fields while absent.
jq -e '(.dropin.semantic_limits|length)==5 and .dropin.semantic_limits.sha256==.dropin.sha256' "$work/lab-a-active-baseline.json" >/dev/null
jq -e '(.dropin.semantic_limits|length)==4 and (.dropin.semantic_limits|has("sha256")|not)' "$work/lab-a-pre-activation.json" >/dev/null

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
reject_error() { local label=$1 filter=$2 stage=$3 expected=$4; jq "$filter" "$work/lab-a-$stage.json" > "$work/bad.json"; sidecar "$work/bad.json"; local args=("${host_args[@]}"); case $stage in pre-activation) args[3]="$work/bad.json";; active-baseline) args[5]="$work/bad.json";; converged) args[7]="$work/bad.json";; post-cleanup) args[9]="$work/bad.json";; esac; if "$root/compare-evidence.py" "${args[@]}" > "$work/bad-report.json"; then echo "accepted $label" >&2; exit 1; fi; [ -s "$work/bad-report.json" ] || { echo "$label: the comparator produced no report at all, which is a crash and not a refusal" >&2; exit 1; }; jq -e --arg e "$expected" '.status=="FAIL" and (.error|contains($e))' "$work/bad-report.json" >/dev/null || { echo "$label was refused for another reason: $(cat "$work/bad-report.json")" >&2; exit 1; }; }
reject_error 'active drop-in without validated hash' 'del(.dropin.semantic_limits.sha256)' active-baseline 'dropin.semantic_limits: unsafe shape'
reject_error 'malformed validated drop-in hash' '.dropin.semantic_limits.sha256="not-a-sha256"' active-baseline 'dropin.semantic_limits.sha256: invalid SHA-256'
reject_error 'validated drop-in hash unbound from installed drop-in' '.dropin.semantic_limits.sha256="0000000000000000000000000000000000000000000000000000000000000000"' active-baseline 'validated drop-in hash is not the installed drop-in hash'
reject_error 'absent drop-in carrying a validated hash' '.dropin.semantic_limits.sha256=null' pre-activation 'dropin.semantic_limits: unsafe shape'
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
# A replica that binds an address other than the one its peers advertise for it (a loopback or wildcard bind) must
# fail the endpoint join, and the passing fixtures cannot show that because they hard-code the agreement.
jq '.listeners.endpoint_commitment="sha256:0000000000000000000000000000000000000000000000000000000000000000"' "$work/lab-a-active-baseline.json" > "$work/elsewhere-bound.json"
sidecar "$work/elsewhere-bound.json"
if "$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work/elsewhere-bound.json" "$work/lab-b-active-baseline.json" "$work/lab-c-active-baseline.json" --converged "$work"/*-converged.json --cleanup "$work"/*-post-cleanup.json > "$work/elsewhere-bound-report.json"; then echo 'accepted a listener bound elsewhere than its advertised endpoint' >&2; exit 1; fi
report_says "$work/elsewhere-bound-report.json" '.status=="FAIL" and (.failures | index("peer endpoint does not bind the remote listener") != null)' 'a listener bound elsewhere did not produce a typed verdict'
# A sidecar binds a digest to a file name: the directory it was written in is the producing host's, and the
# evidence must stay verifiable byte-for-byte once copied beside its comparator, so a foreign directory with the
# same file name is accepted and a different file name is refused.
mkdir -p "$work/elsewhere" && cp "$work/lab-a-post-cleanup.json" "$work/elsewhere/lab-a-post-cleanup.json"
printf '%s  %s\n' "$(sha256sum "$work/elsewhere/lab-a-post-cleanup.json" | awk '{print $1}')" "/another/host/directory/lab-a-post-cleanup.json" > "$work/elsewhere/lab-a-post-cleanup.json.sha256"
args=("${host_args[@]}"); args[9]="$work/elsewhere/lab-a-post-cleanup.json"
"$root/compare-evidence.py" "${args[@]}" > "$work/relocated.json"
jq -e '.status=="PASS"' "$work/relocated.json" >/dev/null
printf '%s  %s\n' "$(sha256sum "$work/elsewhere/lab-a-post-cleanup.json" | awk '{print $1}')" "/another/host/directory/lab-b-post-cleanup.json" > "$work/elsewhere/lab-a-post-cleanup.json.sha256"
if "$root/compare-evidence.py" "${args[@]}" > "$work/misnamed-sidecar.json"; then echo 'accepted a sidecar naming another file' >&2; exit 1; fi
report_says "$work/misnamed-sidecar.json" '.status=="FAIL" and (.error|contains("invalid evidence checksum sidecar"))' 'a misnamed sidecar did not produce a typed verdict'
jq '.inspection.logical_history_sha256="0000000000000000000000000000000000000000000000000000000000000000" | .inspection.history_count=4' "$work/lab-a-post-cleanup.json" > "$work/bad-cleanup.json"
sidecar "$work/bad-cleanup.json"
if "$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-converged.json --cleanup "$work/bad-cleanup.json" "$work/lab-b-post-cleanup.json" "$work/lab-c-post-cleanup.json" >/dev/null; then echo 'accepted divergent post-cleanup history' >&2; exit 1; fi

# A fresh host has no canonical store, and a baseline capture must be able to SEAL that
# fact rather than die on it — otherwise the fresh-store campaign this evidence exists to
# serve cannot produce a baseline at all. Three states are typed and all three are tested:
# not inspected (null), inspected and absent, inspected and present. The negatives below
# exist because "absent" must not become a way to smuggle a capture past validation.
# Extracted from capture-host.sh itself rather than written beside it. A hand-written
# literal is a second copy of the collector's list, and it stayed green through the
# revision where the collector's copy fell five fields behind the comparator's.
absent=$(grep -o "{store_present:false[^']*}" "$root/capture-host.sh" | head -1 | jq -cn -f /dev/stdin)
zero='0000000000000000000000000000000000000000000000000000000000000000'
jq ".inspection=$absent | .exchanges=null" "$work/lab-a-pre-activation.json" > "$work/fresh-pre.json"
sidecar "$work/fresh-pre.json"
args=("${host_args[@]}"); args[3]="$work/fresh-pre.json"
"$root/compare-evidence.py" "${args[@]}" > "$work/fresh-baseline.json"
jq -e '.status=="PASS"' "$work/fresh-baseline.json" >/dev/null || { echo 'refused a legitimate fresh-host baseline' >&2; exit 1; }
reject_error 'absent store carrying a history digest' ".inspection=$absent | .inspection.logical_history_sha256=\"$zero\" | .exchanges=null" pre-activation 'absent store carries derived inspection fields'
reject_error 'absent store carrying a count' ".inspection=$absent | .inspection.incomplete_attempt_count=0 | .exchanges=null" pre-activation 'absent store carries derived inspection fields'
reject_error 'present store missing a derived field' '.inspection.history_count=null' active-baseline 'present store is missing derived inspection fields'
reject_error 'inspection without the store_present discriminator' 'del(.inspection.store_present)' active-baseline 'unsafe shape'
reject_error 'non-boolean store_present' '.inspection.store_present="false"' active-baseline 'store_present: must be a boolean'
# The regression this one guards: with the old evaluation order, a converged capture
# reporting an absent store reached `None >= 3` and raised a TypeError. A crash is not a
# refusal — it produces no verdict and no failure list.
#
# ALL THREE converged captures must report absence for this to bite, and that is not a
# detail. With only one absent, the three history digests are {None, h, h}, the set has
# two members, the chain short-circuits on divergence and never reaches the comparison —
# so a one-host version of this test passes against the crashing comparator and proves
# nothing. It was written that way first. With all three absent the set is {None}, size
# one, and evaluation walks straight into the null comparison.
# The exchanges must go with the store. A capture claiming no canonical store while still
# publishing folded exchanges is refused earlier, as a shape error rather than a verdict —
# correctly, but it is not the case under test here.
for hostname in lab-a lab-b lab-c; do jq ".inspection=$absent | .exchanges=null" "$work/$hostname-converged.json" > "$work/$hostname-nostore.json"; sidecar "$work/$hostname-nostore.json"; done
if "$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-nostore.json --cleanup "$work"/*-post-cleanup.json > "$work/nostore-report.json"; then echo 'accepted converged captures reporting no canonical store' >&2; exit 1; fi
report_says "$work/nostore-report.json" '.status=="FAIL" and (.failures | index("converged captures do not prove one complete logical history of at least three events") != null)' 'absent converged stores did not produce a typed verdict'

# The published incomplete-attempt records and folded exchanges. These are what make the
# accounting conditions evaluable at all, so every self-consistency rule over them is
# exercised: a validation nothing tests is a validation nobody has.
attempt='{"attempt_commitment":"sha256:1111111111111111111111111111111111111111111111111111111111111111","nonce_commitment":"sha256:2222222222222222222222222222222222222222222222222222222222222222","nonce_authority":"peer-validated","operation_commitment":null,"direction":"outbound","last_phase":"outbound_request_prepared"}'
reject_error 'attempt count disagreeing with the published list' ".inspection.incomplete_attempts=[$attempt]" converged 'incomplete_attempt_count does not match the published list'
reject_error 'a repeated attempt identity' ".inspection.incomplete_attempts=[$attempt,$attempt] | .inspection.incomplete_attempt_count=2" converged 'repeats an attempt identity'
reject_error 'an outbound attempt with a locally minted nonce' ".inspection.incomplete_attempts=[$attempt | .nonce_authority=\"pre-authentication\"] | .inspection.incomplete_attempt_count=1" converged 'outbound attempt cannot carry a pre-authentication nonce'
reject_error 'unaudited receipt count disagreeing with its list' '.inspection.unaudited_import_receipt_count=1' converged 'unaudited_import_receipt_count does not match'
reject_error 'two folded rows for one nonce' '.exchanges=[.exchanges[0],.exchanges[0]]' converged 'two folded rows for one nonce'
reject_error 'joinable asserted rather than derived' '.exchanges[0].nonce_authority="pre-authentication" | .exchanges[0].joinable=true' converged 'joinable does not follow from nonce_authority'
reject_error 'an outbound exchange with a locally minted nonce' '.exchanges[0].nonce_authority="pre-authentication" | .exchanges[0].joinable=false | .exchanges[0].direction="outbound"' converged 'outbound exchange cannot carry a pre-authentication nonce'
reject_error 'more phases than rows collapsed' '.exchanges[0].row_count=2' converged 'more phases than collapsed rows'
reject_error 'a phase repeated in one folded row' '.exchanges[0].phases_reached=["inbound_request_observed","inbound_request_observed","inbound_import_committed","inbound_reply_prepared"]' converged 'repeats a phase'
reject_error 'exchanges published with no store inspected' ".inspection=$absent | .exchanges=[]" pre-activation 'exchanges published without an inspected present store'
reject_error 'a present store publishing no exchanges' '.exchanges=null' converged 'no exchanges were published'

# The accounting predicate itself: a stranded sender attempt joined to the receiver that
# served it. This is the case the frozen Stage D contract REQUIRES to exist -- a reply lost
# after the destination commits leaves the source holding a prepared, incomplete attempt --
# and the gate that demanded zero such attempts is what this lot withdrew. Nothing below
# erases or fabricates a terminal event to reach a number.
N='sha256:3333333333333333333333333333333333333333333333333333333333333333'
OP='sha256:4444444444444444444444444444444444444444444444444444444444444444'
RQ='sha256:5555555555555555555555555555555555555555555555555555555555555555'
RA=$(jq -r '.inspection.replica_commitment' "$work/lab-a-post-cleanup.json")
RB=$(jq -r '.inspection.replica_commitment' "$work/lab-b-post-cleanup.json")
strand="{\"attempt_commitment\":\"sha256:6666666666666666666666666666666666666666666666666666666666666666\",\"nonce_commitment\":\"$N\",\"nonce_authority\":\"peer-validated\",\"operation_commitment\":\"$OP\",\"direction\":\"outbound\",\"last_phase\":\"outbound_request_prepared\"}"
sent="{\"nonce_commitment\":\"$N\",\"nonce_authority\":\"peer-validated\",\"joinable\":true,\"direction\":\"outbound\",\"phases_reached\":[\"outbound_request_prepared\"],\"row_count\":1,\"peer_commitment\":\"$RB\",\"operation_commitment\":\"$OP\",\"request_sha256_commitment\":\"$RQ\",\"reply_sha256_commitment\":null,\"local_receipt_commitment\":null,\"remote_receipt_commitment\":null,\"request_frame_bytes\":0,\"reply_frame_bytes\":0,\"request_announced_body_bytes\":2731,\"reply_announced_body_bytes\":null,\"outcomes\":[\"incomplete\"],\"replayed\":null}"
served="{\"nonce_commitment\":\"$N\",\"nonce_authority\":\"peer-validated\",\"joinable\":true,\"direction\":\"inbound\",\"phases_reached\":[\"inbound_request_observed\",\"inbound_import_committed\",\"inbound_reply_prepared\",\"inbound_reply_write_observed\"],\"row_count\":4,\"peer_commitment\":\"$RA\",\"operation_commitment\":\"$OP\",\"request_sha256_commitment\":\"$RQ\",\"reply_sha256_commitment\":\"sha256:7777777777777777777777777777777777777777777777777777777777777777\",\"local_receipt_commitment\":\"sha256:8888888888888888888888888888888888888888888888888888888888888888\",\"remote_receipt_commitment\":null,\"request_frame_bytes\":2735,\"reply_frame_bytes\":626,\"request_announced_body_bytes\":2731,\"reply_announced_body_bytes\":622,\"outcomes\":[\"accepted\"],\"replayed\":false}"

# $1 is an optional jq filter applied to the RECEIVER's served row, to break one condition.
build_strand() {
  jq ".inspection.incomplete_attempts=[$strand] | .inspection.incomplete_attempt_count=1 | .exchanges += [$sent] | .inspection.audit_event_count=([.exchanges[].row_count]|add) | .inspection.imported_operation_commitments=[\"$OP\"]" "$work/lab-a-post-cleanup.json" > "$work/sa.json"; sidecar "$work/sa.json"
  jq ".exchanges += [$served${1:+ | $1}] | .inspection.audit_event_count=([.exchanges[].row_count]|add) | .inspection.imported_operation_commitments=[\"$OP\"]" "$work/lab-b-post-cleanup.json" > "$work/sb.json"; sidecar "$work/sb.json"
  jq ".inspection.imported_operation_commitments=[\"$OP\"]" "$work/lab-c-post-cleanup.json" > "$work/sc0.json"; sidecar "$work/sc0.json"
  "$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-converged.json --cleanup "$work/sa.json" "$work/sb.json" "$work/sc0.json" > "$work/strand-report.json" 2>"$work/strand-err.txt"
}
build_strand
report_says "$work/strand-report.json" '.status=="PASS" and .incomplete_attempt_accounting.accounted_incomplete_attempts==1 and .incomplete_attempt_accounting.unaccounted_incomplete_attempts==0' 'a stranded attempt joined to the receiver that served it was not accounted for'

# One refusal per condition. Each must fail for its OWN reason: an aggregate that could
# compensate for a missing condition is exactly what the predicate forbids.
strand_refuse() {
  local label=$1 filter=$2 expected=$3
  if build_strand "$filter"; then echo "accepted $label" >&2; exit 1; fi
  report_says "$work/strand-report.json" '.status=="FAIL"' "$label did not produce a verdict"
  grep -q "$expected" <(jq -r '(.failures[]?), (.error // empty)' "$work/strand-report.json") || { echo "$label was refused for another reason: $(jq -c '.failures' "$work/strand-report.json")" >&2; exit 1; }
}
strand_refuse 'no receiver bears the nonce'        '.nonce_commitment="sha256:9999999999999999999999999999999999999999999999999999999999999999" | .joinable=true' 'no receiver-side request bears this wire nonce'
strand_refuse 'the receiver authenticated another sender' '.peer_commitment="sha256:aaaa999999999999999999999999999999999999999999999999999999999999"' 'did not authenticate the sending host as its peer'
strand_refuse 'the operation does not bind'        '.operation_commitment="sha256:bbbb999999999999999999999999999999999999999999999999999999999999"' 'operation ID does not bind sender to receiver'
strand_refuse 'the request digest does not bind'   '.request_sha256_commitment="sha256:cccc999999999999999999999999999999999999999999999999999999999999"' 'request digest does not bind sender to receiver'
strand_refuse 'the announced size does not bind'   '.request_announced_body_bytes=999 | .request_frame_bytes=1003' 'announced request size does not bind'
strand_refuse 'the request frame is short'         '.request_frame_bytes=2700' 'did not record a complete request frame'
strand_refuse 'no import and no typed refusal'     '.phases_reached=["inbound_request_observed","inbound_reply_prepared","inbound_reply_write_observed"] | .row_count=3' 'committed neither an import receipt nor a typed refusal'
strand_refuse 'an import with no receipt'          '.local_receipt_commitment=null' 'committed an import with no receipt'
strand_refuse 'the reply was never written'        '.reply_frame_bytes=0' 'did not write a reply at all'
# "Completely wrote the correctly bound reply" has two halves. Checking only that bytes
# left the receiver passes a truncated write and an unbound one alike.
strand_refuse 'the reply is not bound by a digest'  '.reply_sha256_commitment=null' 'reply is not bound by a digest'
strand_refuse 'the reply was truncated'             '.reply_frame_bytes=600' 'did not completely write the reply it announced'

# Four conditions the nine refusals above do not reach, found by weakening the comparator
# and watching the suite stay green. A refusal path nothing exercises is a refusal path
# nobody has, and three of these guard the clauses the predicate is most about.
strand_three() {   # $1 applied to lab-a, $2 to lab-b's served row, $3 to lab-c
  jq ".inspection.incomplete_attempts=[$strand] | .inspection.incomplete_attempt_count=1 | .exchanges += [$sent] | .inspection.audit_event_count=([.exchanges[].row_count]|add) | .inspection.imported_operation_commitments=[\"$OP\"]${1:+ | $1}" "$work/lab-a-post-cleanup.json" > "$work/sa.json"; sidecar "$work/sa.json"
  jq ".exchanges += [$served${2:+ | $2}] | .inspection.audit_event_count=([.exchanges[].row_count]|add) | .inspection.imported_operation_commitments=[\"$OP\"]" "$work/lab-b-post-cleanup.json" > "$work/sb.json"; sidecar "$work/sb.json"
  jq ".inspection.imported_operation_commitments=[\"$OP\"]${3:+ | $3} | .inspection.audit_event_count=([.exchanges[].row_count]|add)" "$work/lab-c-post-cleanup.json" > "$work/sc.json"; sidecar "$work/sc.json"
  "$root/compare-evidence.py" --phase three-host --pre "$work"/*-pre-activation.json --active-baseline "$work"/*-active-baseline.json --converged "$work"/*-converged.json --cleanup "$work/sa.json" "$work/sb.json" "$work/sc.json" > "$work/strand-report.json" 2>"$work/strand-err.txt"
}
three_refuse() {
  local label=$1 fa=$2 fb=$3 fc=$4 expected=$5
  if strand_three "$fa" "$fb" "$fc"; then echo "accepted $label" >&2; exit 1; fi
  report_says "$work/strand-report.json" '.status=="FAIL"' "$label did not produce a verdict"
  grep -q "$expected" <(jq -r '(.failures[]?), (.error // empty)' "$work/strand-report.json") || { echo "$label was refused for another reason: $(jq -c '.failures' "$work/strand-report.json")" >&2; exit 1; }
}
# Condition 2's uniqueness: two hosts claiming to have served one wire nonce means neither
# can be believed, and the attempt is unaccounted rather than accounted twice.
three_refuse 'two hosts claim the receiver side' '' '' ".exchanges += [$served]" 'more than one host claims the receiver side'
# Condition 5's honest retention: the sender must still be holding the attempt as prepared.
# A sender claiming a later phase while remaining incomplete is not the shape the contract
# describes, and the join must not paper over it.
three_refuse 'the sender did not honestly retain the absence' '.inspection.incomplete_attempts[0].last_phase="outbound_exchange_completed"' '' '' 'an incomplete attempt cannot have a terminal phase'
# Condition 4: an unaudited import receipt anywhere puts the accounting evidence itself in
# question, so no attempt is accounted for while one exists.
three_refuse 'an unaudited import receipt exists' '' '' '.inspection.unaudited_import_receipt_count=1 | .inspection.unaudited_import_receipt_commitments=["sha256:dddd999999999999999999999999999999999999999999999999999999999999"]' 'unaudited import receipts exist'
# Condition 6: with no replayed retry and no converged canonical history, eventual
# convergence cannot stand in for a per-attempt proof.
# Condition 6, both branches. It used to read "a replayed retry OR the histories converge",
# and the convergence half was the same digest equality the gate already asserts for any
# campaign that reaches this point -- so the condition could never refuse. It now asks what
# the condition asks: is THIS operation held, with a receipt, on EVERY replica. One replica
# missing it, and no other host observing a replay, is unaccounted.
three_refuse 'the operation is not held on every replica' '' '' '.inspection.imported_operation_commitments=[]' 'not held with a receipt on every replica'
# And the replay must be observed by someone else: a sender asserting `replayed` on its own
# outbound row closed the condition by itself before.
three_refuse 'the sender asserts its own replay' '.exchanges[-1].replayed=true' '' '.inspection.imported_operation_commitments=[]' 'not held with a receipt on every replica'

# The forgeries an independent review demonstrated against this comparator. Each check that
# closes one is exercised here, because implementing a check and never testing it leaves the
# suite green while the hole is reopened.
two_on_one="[$strand, ($strand | .attempt_commitment=\"sha256:eeee999999999999999999999999999999999999999999999999999999999999\")]"
reject_error 'two attempts sharing one wire nonce' ".inspection.incomplete_attempts=$two_on_one | .inspection.incomplete_attempt_count=2 | .inspection.audit_event_count=9" converged 'two incomplete attempts share one wire nonce'
reject_error 'attempts reported with no audit events' ".inspection.incomplete_attempts=[$strand] | .inspection.incomplete_attempt_count=1 | .inspection.audit_event_count=0" converged 'no audit events to derive them from'
three_refuse 'the attempt names another operation' '.inspection.incomplete_attempts[0].operation_commitment="sha256:ffff999999999999999999999999999999999999999999999999999999999999"' '' '' 'name different operations'
three_refuse 'an import committed without an accepted outcome' '' '.outcomes=["authenticated_refusal"]' '' 'import without an accepted outcome'
three_refuse 'a refusal recorded beside an accepted outcome' '' '.phases_reached=["inbound_request_observed","inbound_refusal_recorded","inbound_reply_prepared","inbound_reply_write_observed"] | .local_receipt_commitment=null' '' 'refusal and an accepted outcome at once'

# Exchanges must account for the whole audit history: every audit row belongs to exactly
# one folded exchange. Without this a host publishes an empty exchange list beside a
# non-zero audit count and nothing binds the two.
three_refuse 'exchanges that do not account for the audit history' '.inspection.audit_event_count += 3' '' '' 'collapse'
# And the framing overhead is the protocol constant, not whatever the campaign agrees on:
# deriving it from the rows under test made the per-row comparison unable to fail.
three_refuse 'frames not carrying the protocol overhead' '' '.request_frame_bytes=2827 | .request_announced_body_bytes=2731' '' 'do not carry the protocol framing overhead'

# The forgeries a second independent review demonstrated. Finding 3 survived the first
# correction: refusing "attempts with zero audit events" was defeated by raising that one
# integer, so a host still listed a brand-new attempt in its own baseline and retired it as
# pre-existing debt, PASS with no failures.
three_refuse 'an attempt conjured into a capture with no row of its own' '.inspection.incomplete_attempts[0].nonce_commitment="sha256:aaaa111111111111111111111111111111111111111111111111111111111111"' '' '' 'no exchange row in its own capture'
# row_count was bounded from below only, so a host could absorb a competing receiver row
# into another row's count and keep the collapsed total equal to its reported audit count.
three_refuse 'a folded row absorbing more rows than a nonce can have' '' '.row_count=8' '' 'cannot collapse more than four audit rows'
# Condition 6 rests on this list, and it was bound to nothing the same host publishes.
three_refuse 'more imported operations than receipts to hold them' '' '' '.inspection.imported_operation_commitments=["sha256:bbbb111111111111111111111111111111111111111111111111111111111111","sha256:cccc111111111111111111111111111111111111111111111111111111111111","sha256:dddd111111111111111111111111111111111111111111111111111111111111","sha256:eeee111111111111111111111111111111111111111111111111111111111111"]' 'more imported operations than receipts'
three_refuse 'a repeated imported operation' '' '' '.inspection.imported_operation_commitments=["sha256:bbbb111111111111111111111111111111111111111111111111111111111111","sha256:bbbb111111111111111111111111111111111111111111111111111111111111"]' 'repeats an operation'
# The campaign-wide reply framing check is the only one that reaches a row no attempt joins.
three_refuse 'a reply frame not carrying the protocol overhead' '' '' '.exchanges[0].reply_frame_bytes=999' 'reply frames do not carry the protocol framing overhead'
# An empty reply body satisfied "completely written" arithmetically.
three_refuse 'an empty reply body counted as written' '' '.reply_announced_body_bytes=0 | .reply_frame_bytes=4' '' 'did not completely write the reply it announced'

# The salted drop-in digest closes a confirmation oracle for the peer address pair, and
# nothing exercised the salted path: the suite called validate-dropin.py without a salt, so
# reverting the salting left both suites green.
saltfile="$work/salt.bin"; head -c 64 /dev/urandom > "$saltfile"
salted=$(python3 "$root/validate-dropin.py" --dropin "$work/good.conf" --salt-file "$saltfile" | jq -r .sha256)
bare=$(sha256sum -- "$work/good.conf" | awk '{print $1}')
expect=$( { cat "$saltfile"; printf '\000dropin\000'; cat "$work/good.conf"; } | sha256sum | awk '{print $1}')
[ "$salted" = "$expect" ] || { echo 'the validator and the collector disagree on the salted drop-in commitment' >&2; exit 1; }
[ "$salted" != "$bare" ] || { echo 'the published drop-in digest is the raw digest, which confirms the peer address pair' >&2; exit 1; }

# The evidence checksum. A third review found it removable with both suites green, which
# makes it the most important omission in the file: without it a tampered body passes under
# its original sidecar and every other check in this comparator is reasoning about a
# document nobody verified.
cp "$work/lab-a-post-cleanup.json" "$work/tampered.json"; cp "$work/lab-a-post-cleanup.json.sha256" "$work/tampered.json.sha256"
sed -i 's/lab-a-post-cleanup.json/tampered.json/' "$work/tampered.json.sha256"
jq '.inspection.history_count=4' "$work/lab-a-post-cleanup.json" > "$work/tampered.json"
args=("${host_args[@]}"); args[9]="$work/tampered.json"
if "$root/compare-evidence.py" "${args[@]}" > "$work/tampered-report.json" 2>/dev/null; then echo 'accepted evidence edited under its original checksum' >&2; exit 1; fi
report_says "$work/tampered-report.json" '.status=="FAIL" and (.error|contains("evidence checksum mismatch"))' 'a tampered body was refused for another reason'

# Other load-bearing checks a review found untested. Each is the only home of the property
# it guards.
reject_error 'a store failing its integrity check' '.inspection.sqlite_integrity_result="corrupt"' converged 'invalid read-only inspection'
reject_error 'a package the host cannot prove clean' '.package.dpkg_verify="modified"' converged 'candidate is not proven installed and clean'
# This one refuses through the failures list rather than a validation error, so it is
# asserted with reject_host and its message checked separately below.
reject_host 'an inspection bound to another replica' '.inspection.replica_commitment="sha256:9999111111111111111111111111111111111111111111111111111111111111"'

# The corroboration rule cost a forger nothing: any row bearing the nonce satisfied it.
junk="{\"nonce_commitment\":\"$N\",\"nonce_authority\":\"peer-validated\",\"joinable\":true,\"direction\":\"outbound\",\"phases_reached\":[\"outbound_request_prepared\"],\"row_count\":1,\"peer_commitment\":null,\"operation_commitment\":null,\"request_sha256_commitment\":null,\"reply_sha256_commitment\":null,\"local_receipt_commitment\":null,\"remote_receipt_commitment\":null,\"request_frame_bytes\":0,\"reply_frame_bytes\":0,\"request_announced_body_bytes\":null,\"reply_announced_body_bytes\":null,\"outcomes\":[\"accepted\"],\"replayed\":null}"
reject_error 'an attempt corroborated by a row that is not a stranded attempt' ".inspection.incomplete_attempts=[$strand] | .inspection.incomplete_attempt_count=1 | .exchanges=[$junk] | .inspection.audit_event_count=1" converged 'not a stranded attempt'
reject_error 'an attempt and its row naming different operations' ".inspection.incomplete_attempts=[$strand] | .inspection.incomplete_attempt_count=1 | .exchanges=[$sent | .operation_commitment=\"sha256:7777111111111111111111111111111111111111111111111111111111111111\"] | .inspection.audit_event_count=1" converged 'name different operations'
# And the converse: a stranded row simply omitted from the list. Concealing an attempt was
# cheaper than the forgery the corroboration rule was written to stop.
reject_error 'a stranded row concealed from the attempt list' ".exchanges=[$sent] | .inspection.audit_event_count=1" converged 'missing from the incomplete-attempt list'

printf '%s\n' 'PASS: strand accounting — one stranded attempt joined to its receiver, and eleven conditions each refused on its own.'

printf '%s\n' 'PASS: four-stage activation evidence, effective policy, ownership, graceful cleanup, infrastructure stability, converged history boundaries, typed fresh-store absence, published incomplete attempts and folded exchanges.'
