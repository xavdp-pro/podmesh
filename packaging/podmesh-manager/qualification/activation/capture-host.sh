#!/bin/bash
# Read-only local activation evidence collector. It never contacts another host.
set -euo pipefail
export LC_ALL=C
umask 077

root=$(cd -- "$(dirname -- "$0")" && pwd)
usage() { echo "Usage: $0 --host-alias <stable-alias> --stage <pre-activation|active-baseline|converged|post-cleanup> --salt-file <private-salt> --candidate-verification <report.json> --output <evidence.json> [--with-inspection] [--shutdown-report <report.json>]" >&2; exit 2; }
alias_name= stage= salt= report= output= shutdown_report= with_inspection=0
while [ "$#" -gt 0 ]; do case "$1" in
  --host-alias) alias_name=${2-}; shift 2;; --stage) stage=${2-}; shift 2;;
  --salt-file) salt=${2-}; shift 2;; --candidate-verification) report=${2-}; shift 2;;
  --output) output=${2-}; shift 2;; --shutdown-report) shutdown_report=${2-}; shift 2;;
  --with-inspection) with_inspection=1; shift;; *) usage;; esac; done
[[ "$alias_name" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$ ]] || usage
case "$stage" in pre-activation|active-baseline|converged|post-cleanup) ;; *) usage;; esac
[ "$(id -u)" -eq 0 ] && [ -f "$salt" ] && [ -f "$report" ] && [ -n "$output" ] || usage
[ "$stage" != post-cleanup ] || [ -f "$shutdown_report" ] || usage
[ -z "$shutdown_report" ] || [ "$stage" = post-cleanup ] || usage
[ "$(stat -c '%a:%u:%F' -- "$salt")" = '600:0:regular file' ] && [ "$(stat -c '%s' -- "$salt")" -ge 32 ] || { echo 'Salt must be root-owned mode 0600 with at least 32 bytes' >&2; exit 2; }
for command in jq systemctl dpkg-query dpkg sha256sum stat find sort sed podman ip nft ss getent awk readlink tr runuser python3; do command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }; done
work=$(mktemp -d); trap 'rm -rf -- "$work"' EXIT

# A label names the kind of value, never the place it was observed: the comparator joins a replica ID, a logical
# manager ID or an endpoint across configuration, peers, listeners and inspection, and two labels for one value
# would make its commitments unrelated.
commit_stdin() { { cat -- "$salt"; printf '\000%s\000' "$1"; cat; } | sha256sum | awk '{print "sha256:" $1}'; }
commit_file() { { cat -- "$salt"; printf '\000%s\000' "$1"; cat -- "$2"; } | sha256sum | awk '{print "sha256:" $1}'; }
commit_text() { printf '%s' "$2" | commit_stdin "$1"; }
# Commit many values in one pass and return a jq object keyed "<label>\t<raw value>", which
# is what fold-exchanges.jq looks values up in. A shell loop rather than a script, to keep
# this collector's dependency list as it is: a few hundred digests on a capture that already
# runs dpkg --verify. Values are validated tokens in the candidate's schema, but a tab or a
# newline in one would silently corrupt the map, so it refuses instead of guessing.
commit_batch() {
  local kind value
  while IFS=$'\t' read -r kind value; do
    [ -n "$kind" ] && [ -n "$value" ] || continue
    case $value in *"$(printf '\t')"*) echo "Refusing to commit a value containing a tab" >&2; return 1;; esac
    printf '%s\t%s\t%s\n' "$kind" "$value" "$(commit_text "$kind" "$value")"
  done | jq -Rn '[inputs | split("\t") | select(length == 3) | {key: (.[0] + "\t" + .[1]), value: .[2]}] | from_entries'
}
# Every raw value the inspection projection can be asked to commit, one "<label>\t<value>"
# line each. Labels name the kind of value and are established from the candidate's own
# assignments and comparisons; fold-exchanges.jq records which line proves which.
commitment_keys() {
  jq -r '
    [ (.incomplete_attempts[] | [["attempt-id",.attempt_id],["wire-nonce",.wire_nonce],["wire-operation-id",.wire_operation_id]]),
      (.ordered_audit_events[].event | [["attempt-id",.attempt_id],["wire-nonce",.wire_nonce],["replica-id",.authenticated_peer_id],
                                        ["replica-id",.peer_claim],["wire-operation-id",.operation_id],
                                        ["request-digest",.request_sha256],["reply-digest",.reply_sha256],
                                        ["receipt-digest",.local_receipt_sha256],["receipt-digest",.remote_receipt_sha256],
                                        ["receipt-operation-id",.local_receipt_operation_id],["receipt-operation-id",.remote_receipt_operation_id]]),
      (.ordered_receipts | map([["receipt-operation-id", .operation_id], ["wire-operation-id", .wire_operation_id], ["receipt-digest", .sha256], ["replica-id", .source_replica_id]]) | add),
      (.unaudited_import_receipt_ids | map(["receipt-operation-id", .]))
    ] | add | map(select(.[1] != null)) | map(.[0] + "\t" + .[1]) | unique | .[]'
}
metadata() {
  local path=$1
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then jq -cn '{present:false,uid:null,gid:null,mode:null,content_commitment:null}'; return; fi
  [ ! -L "$path" ] || { echo "Refusing symlinked protected path $path" >&2; return 1; }
  local uid gid mode material
  read -r uid gid mode < <(stat -c '%u %g %a' -- "$path")
  material=$(while IFS= read -r -d '' relative; do
    target=$path${relative:+/$relative}
    stat -c '%n\t%F\t%a\t%u\t%g\t%s' -- "$target"
    if [ -f "$target" ] && [ ! -L "$target" ]; then sha256sum -- "$target"; fi
  done < <(find -P "$path" -xdev -printf '%P\0' | sort -z) | commit_stdin "path:$path")
  jq -cn --argjson uid "$uid" --argjson gid "$gid" --arg mode "$mode" --arg commitment "$material" '{present:true,uid:$uid,gid:$gid,mode:$mode,content_commitment:$commitment}'
}
socket_metadata() {
  local path=$1
  if [ ! -e "$path" ] && [ ! -L "$path" ]; then jq -cn '{present:false,uid:null,gid:null,mode:null}'; return; fi
  [ -S "$path" ] && [ ! -L "$path" ] || { echo "Manager control socket is not a socket: $path" >&2; return 1; }
  local uid gid mode; read -r uid gid mode < <(stat -c '%u %g %a' -- "$path")
  jq -cn --argjson uid "$uid" --argjson gid "$gid" --arg mode "$mode" '{present:true,uid:$uid,gid:$gid,mode:$mode}'
}
observe() {
  local label=$1; shift
  local raw
  if ! raw=$("$@" 2>"$work/$label.err"); then jq -cn '{status:"unknown",commitment:null}'; return; fi
  jq -cn --arg commitment "$(printf '%s' "$raw" | commit_stdin "$label")" '{status:"available-successful",commitment:$commitment}'
}
unit() {
  local name=$1 raw invocation
  raw=$(systemctl show "$name" --no-page -p LoadState -p ActiveState -p SubState -p UnitFileState -p MainPID -p InvocationID -p NRestarts -p Result -p ExecMainCode -p ExecMainStatus) || return 1
  invocation=$(awk -F= '$1=="InvocationID" {print substr($0,index($0,"=")+1)}' <<<"$raw")
  jq -Rn --arg raw "$raw" --arg invocation "${invocation:+$(commit_text invocation "$invocation")}" '
    reduce ($raw|split("\n")[]|select(length>0)|split("=")|{k:.[0],v:(.[1:]|join("="))}) as $p
      ({load_state:null,active_state:null,sub_state:null,unit_file_state:null,main_pid:null,n_restarts:null,result:null,exec_main_code:null,exec_main_status:null};
       if $p.k=="LoadState" then .load_state=$p.v elif $p.k=="ActiveState" then .active_state=$p.v elif $p.k=="SubState" then .sub_state=$p.v elif $p.k=="UnitFileState" then .unit_file_state=$p.v elif $p.k=="MainPID" then .main_pid=($p.v|tonumber) elif $p.k=="NRestarts" then .n_restarts=($p.v|tonumber) elif $p.k=="Result" then .result=$p.v elif $p.k=="ExecMainCode" then .exec_main_code=$p.v elif $p.k=="ExecMainStatus" then .exec_main_status=($p.v|tonumber) else . end) |
    .invocation_commitment=(if $invocation=="" then null else $invocation end)'
}
manager_process() {
  local pid=$1 found uid argv count candidate
  found=()
  for candidate in /proc/[0-9]*; do
    [ -e "$candidate/exe" ] || continue
    [ "$(readlink -f -- "$candidate/exe" 2>/dev/null || true)" = /usr/lib/podmesh-manager/podmesh-managerd ] || continue
    found+=("${candidate##*/}")
  done
  count=${#found[@]}
  if [ "$count" -eq 0 ]; then jq -cn '{count:0,pid:null,uid:null,argv_commitment:null,argv_count:0}'; return; fi
  [ "$count" -eq 1 ] && [ "${found[0]}" = "$pid" ] || { echo 'Manager process count/PID disagrees with systemd' >&2; return 1; }
  uid=$(stat -c '%u' -- "/proc/$pid")
  argv=$(tr '\000' '\n' < "/proc/$pid/cmdline")
  jq -cn --argjson pid "$pid" --argjson uid "$uid" --arg commitment "$(printf '%s' "$argv" | commit_stdin manager-argv)" --argjson argc "$(awk 'NF {n++} END {print n+0}' <<<"$argv")" '{count:1,pid:$pid,uid:$uid,argv_commitment:$commitment,argv_count:$argc}'
}
configuration() {
  local path=/etc/podmesh-manager/config.json normalized peers topology
  [ -f "$path" ] && [ ! -L "$path" ] || { echo 'Protected manager configuration is missing or symlinked' >&2; return 1; }
  normalized=$(jq -ceS '
    . as $root | .network as $network | ($network.manager.replicas|sort_by(.replica_id,.host_id)) as $replicas |
    ($network.manager.grants|sort_by(.scope,.owner_replica_id)) as $grants |
    ($replicas[]|select(.replica_id==$network.replica_id)|.host_id) as $host |
    {logical:$network.manager.logical_manager_id,replica:$network.replica_id,host:$host,topology:{replicas:$replicas,grants:$grants},peers:($network.peers|sort_by(.replica_id)|map({replica_id,endpoint,shared_key_hex}))}' "$path") || return 1
  peers='[]'
  while IFS=$'\t' read -r replica endpoint key; do
    peers=$(jq -c --arg replica "$(commit_text replica-id "$replica")" --arg endpoint "$(commit_text endpoint "$endpoint")" --arg key "$(commit_text peer-key "$key")" '.+[{replica_id_commitment:$replica,endpoint_commitment:$endpoint,shared_key_commitment:$key}]' <<<"$peers")
  done < <(jq -r '.peers[]|[.replica_id,.endpoint,.shared_key_hex]|@tsv' <<<"$normalized")
  topology=$(jq -cS .topology <<<"$normalized")
  jq -cn --arg document "$(commit_file config-document "$path")" --arg logical "$(commit_text logical-manager-id "$(jq -r .logical <<<"$normalized")")" --arg replica "$(commit_text replica-id "$(jq -r .replica <<<"$normalized")")" --arg host "$(commit_text local-host "$(jq -r .host <<<"$normalized")")" --arg topology "$(printf '%s' "$topology" | commit_stdin topology)" --argjson peers "$peers" '{document_commitment:$document,logical_manager_commitment:$logical,local_replica_commitment:$replica,local_host_commitment:$host,topology_commitment:$topology,peer_count:($peers|length),peers:$peers}'
}
dropin() {
  local installed=/etc/systemd/system/podmesh-manager.service.d/90-g2-network.conf packaged=/usr/lib/systemd/system/podmesh-manager.service paths
  [ -f "$packaged" ] && grep -Fxq 'IPAddressDeny=any' "$packaged" || { echo 'Packaged deny-all policy is absent' >&2; return 1; }
  paths=$(systemctl show podmesh-manager.service --no-page -p DropInPaths --value) || return 1
  if [ -n "$paths" ] && [ "$paths" != "$installed" ]; then echo 'Undeclared manager systemd drop-in is present' >&2; return 1; fi
  if [ -e "$installed" ]; then
    [ -f "$installed" ] && [ ! -L "$installed" ] || { echo 'Activation drop-in is unsafe' >&2; return 1; }
    local limits environment families allows denies expected_allow actual_allow policy
    limits=$(python3 "$root/validate-dropin.py" --dropin "$installed" --salt-file "$salt") || return 1
    environment=$(systemctl show podmesh-manager.service -p Environment --value) || return 1
    families=$(systemctl show podmesh-manager.service -p RestrictAddressFamilies --value) || return 1
    allows=$(systemctl show podmesh-manager.service -p IPAddressAllow --value) || return 1
    denies=$(systemctl show podmesh-manager.service -p IPAddressDeny --value) || return 1
    grep -Eq '(^| )PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers( |$)' <<<"$environment" || { echo 'Effective network mode is not authenticated-static-peers' >&2; return 1; }
    [ "$(tr ' ' '\n' <<<"$families" | sed '/^$/d' | sort | tr '\n' ' ')" = 'AF_INET AF_UNIX ' ] || { echo 'Effective address families differ from AF_UNIX and AF_INET' >&2; return 1; }
    expected_allow=$(jq -r '.network.peers[].endpoint | capture("^(?<address>[0-9]+\\.[0-9]+\\.[0-9]+\\.[0-9]+):[0-9]+$").address + "/32"' /etc/podmesh-manager/config.json | sort | tr '\n' ' ')
    actual_allow=$(tr ' ' '\n' <<<"$allows" | sed '/^$/d' | sort | tr '\n' ' ')
    [ "$actual_allow" = "$expected_allow" ] || { echo 'Effective IP allow-list differs from configured peers' >&2; return 1; }
    case "$(tr ' ' '\n' <<<"$denies" | sed '/^$/d' | sort | tr '\n' ' ')" in
      '0.0.0.0/0 ::/0 '|'any ') ;;
      *) echo 'Effective deny-all policy is absent' >&2; return 1;;
    esac
    policy=$(printf '%s\0%s\0%s\0%s' "$environment" "$families" "$allows" "$denies" | commit_stdin effective-unit-network-policy)
    jq -cn --arg sha "$(commit_file dropin "$installed" | sed 's/^sha256://')" --arg fragment "$(sha256sum -- "$packaged"|awk '{print $1}')" --arg policy "$policy" --argjson limits "$limits" '{present:true,sha256:$sha,semantic_limits:$limits,packaged_fragment_sha256:$fragment,inherited_deny_all:true,effective_policy_configured:true,effective_policy_commitment:$policy}'
  else
    jq -cn --arg fragment "$(sha256sum -- "$packaged"|awk '{print $1}')" '{present:false,sha256:null,semantic_limits:{network_mode:null,address_families:[],peer_allow_count:0,peer_allow_prefix_length:null},packaged_fragment_sha256:$fragment,inherited_deny_all:true,effective_policy_configured:false,effective_policy_commitment:null}'
  fi
}
listeners() {
  local endpoint port pid tcp_raw udp_raw tcp udp
  endpoint=$(jq -r '.network.bind' /etc/podmesh-manager/config.json); port=${endpoint##*:}; [[ "$port" =~ ^[0-9]+$ ]] || return 1
  pid=$(systemctl show podmesh-manager.service -p MainPID --value)
  tcp_raw=$(ss -H -ltnp "sport = :$port") || return 1
  udp_raw=$(ss -H -lunp "sport = :$port") || return 1
  tcp=$(awk -v endpoint="$endpoint" -v pid="$pid" '$4==endpoint && index($0,"pid=" pid ",") {n++} END{print n+0}' <<<"$tcp_raw")
  udp=$(awk 'NF {n++} END{print n+0}' <<<"$udp_raw")
  jq -cn --arg endpoint "$(commit_text endpoint "$endpoint")" --argjson tcp "$tcp" --argjson udp "$udp" '{status:"available-successful",endpoint_commitment:$endpoint,tcp_listener_count:$tcp,udp_listener_count:$udp}'
}
# Three states, not two. `null` means this capture did not inspect at all;
# store_present:false means it inspected and there is no canonical store yet;
# store_present:true means it inspected one. A fresh host is the ordinary starting
# condition of a campaign, so its baseline must be sealable rather than fatal.
#
# Absence is PROVEN by looking for the file, never inferred from a failed inspection.
# That distinction is the whole safety of this function: a corrupt store, an unreadable
# one, a refused schema version, or a symlink where a database belongs must all stay
# fatal. If absence were inferred from failure, any of those would be laundered into
# "this host is fresh" and the baseline set would silently become empty — which is
# exactly the fail-open the accounting predicate cannot survive.
inspection() {
  [ "$with_inspection" -eq 1 ] || { printf '%s\n' null; return; }
  local store=/var/lib/podmesh-manager/store.sqlite raw
  if [ ! -e "$store" ] && [ ! -L "$store" ]; then
    # The installed package creates the state directory; only the store itself is
    # created on first run. A missing directory is therefore not a fresh host, it is a
    # host that is not in the state this capture assumes, and it stays fatal.
    [ -d /var/lib/podmesh-manager ] && [ ! -L /var/lib/podmesh-manager ] || { echo 'Manager state directory is absent or is not a directory' >&2; return 1; }
    # EVERY derived field, null. The list grew when incomplete attempts and the two set
    # digests were published and this branch did not grow with it, so a genuinely fresh host
    # emitted ten keys where the comparator required fifteen and the whole campaign was
    # refused with a shape error naming nothing — defeating the very case this branch exists
    # to serve. The comparator's DERIVED tuple and this literal are one list in two places.
    jq -cn '{store_present:false,schema_version:null,logical_manager_commitment:null,replica_commitment:null,logical_history_sha256:null,receipt_set_sha256:null,audit_set_sha256:null,sqlite_integrity_result:null,history_count:null,receipt_count:null,audit_event_count:null,incomplete_attempt_count:null,incomplete_attempts:null,unaudited_import_receipt_count:null,unaudited_import_receipt_commitments:null,imported_operation_commitments:null}'
    return
  fi
  raw=$(runuser -u podmesh-manager -- /usr/lib/podmesh-manager/podmesh-managerd --inspect-store --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager) || { echo 'Read-only canonical inspection failed' >&2; return 1; }
  # The store is read ONCE. Two projections derive from that single read, so the published
  # inspection and the published exchanges can never describe two different moments.
  printf '%s' "$raw" > "$work/inspection-raw.json"
  commitment_keys < "$work/inspection-raw.json" | commit_batch > "$work/commitments.json" || return 1
  jq -ce --arg logical "$(commit_text logical-manager-id "$(jq -r .logical_manager_id <<<"$raw")")" \
     --arg replica "$(commit_text replica-id "$(jq -r .replica_id <<<"$raw")")" \
     --argjson commitments "$(cat "$work/commitments.json")" '
    def c($kind; $v): if $v == null then null else ($commitments[$kind + "\t" + $v] // error("no commitment for " + $kind)) end;
    {store_present:true,schema_version,logical_manager_commitment:$logical,replica_commitment:$replica,
     logical_history_sha256,receipt_set_sha256,audit_set_sha256,sqlite_integrity_result,
     history_count,receipt_count,audit_event_count,
     incomplete_attempt_count:(.incomplete_attempts|length),
     # The list, not only its length. Folding it to an integer is what made six of the
     # eight accounting conditions impossible to evaluate from sealed evidence.
     incomplete_attempts:[.incomplete_attempts[] | {
        attempt_commitment: c("attempt-id"; .attempt_id),
        nonce_commitment: c("wire-nonce"; .wire_nonce),
        nonce_authority: (if (.wire_nonce|startswith("preauth:")) then "pre-authentication" else "peer-validated" end),
        operation_commitment: c("wire-operation-id"; .wire_operation_id),
        direction, last_phase}],
     unaudited_import_receipt_count:(.unaudited_import_receipt_ids|length),
     unaudited_import_receipt_commitments:[.unaudited_import_receipt_ids[] | c("receipt-operation-id"; .)],
     # The wire operations this replica holds a receipt for. Without these, the convergence
     # branch of condition 6 has nothing to check but the history digests, which the gate
     # already compares elsewhere -- so the branch could never refuse, and the warning in
     # the predicate that eventual history convergence alone is insufficient was exactly
     # what the code did. (No apostrophes here: this whole jq program is a shell single-
     # quoted string, and one apostrophe ends it mid-filter.)
     imported_operation_commitments:[.ordered_receipts[] | select(.wire_operation_id != null) | c("wire-operation-id"; .wire_operation_id)] | unique}' <<<"$raw"
}
# The receiver side of the join, folded to one row per wire nonce. Present only on a
# capture taken --with-inspection and only when a store exists: absence of a store is not
# an empty exchange list, it is no exchange list at all, and the two must stay
# distinguishable.
exchanges() {
  [ "$with_inspection" -eq 1 ] || { printf '%s\n' null; return; }
  [ -s "$work/inspection-raw.json" ] || { printf '%s\n' null; return; }
  jq -c --argjson commitments "$(cat "$work/commitments.json")" -f "$root/fold-exchanges.jq" "$work/inspection-raw.json"
}

jq -e '.schema_version=="podmesh-manager-candidate-verification/v2" and .package=="podmesh-manager" and (.version|type=="string") and (.binary_sha256|test("^[a-f0-9]{64}$"))' "$report" >/dev/null || { echo 'Candidate verification report is invalid' >&2; exit 2; }
version=$(jq -r .version "$report"); binary_hash=$(jq -r .binary_sha256 "$report")
[ "$(dpkg-query -W -f='${db:Status-Status}' podmesh-manager)" = installed ] && [ "$(dpkg-query -W -f='${Version}' podmesh-manager)" = "$version" ] || { echo 'Installed package differs from candidate report' >&2; exit 2; }
[ "$(sha256sum /usr/lib/podmesh-manager/podmesh-managerd|awk '{print $1}')" = "$binary_hash" ] || { echo 'Installed binary differs from candidate report' >&2; exit 2; }
set +e; verify=$(dpkg --verify podmesh-manager 2>&1); verify_rc=$?; set -e
[ "$verify_rc" -eq 0 ] && [ -z "$verify" ] || { echo 'dpkg verification is not clean' >&2; exit 2; }
account_uid=$(getent passwd podmesh-manager | cut -d: -f3); [[ "$account_uid" =~ ^[1-9][0-9]*$ ]] || { echo 'Manager account is missing or privileged' >&2; exit 2; }
[ -d /var/lib/podmesh-manager ] && [ ! -L /var/lib/podmesh-manager ] || { echo 'Manager state path is missing or unsafe' >&2; exit 2; }
if [ -e /run/podmesh-manager ] || [ -L /run/podmesh-manager ]; then [ -d /run/podmesh-manager ] && [ ! -L /run/podmesh-manager ] || { echo 'Manager runtime path is unsafe' >&2; exit 2; }; fi
service=$(unit podmesh-manager.service); pid=$(jq -r .main_pid <<<"$service")
config=$(configuration); dropin_value=$(dropin); process=$(manager_process "$pid")
state=$(metadata /var/lib/podmesh-manager); runtime=$(metadata /run/podmesh-manager); socket=$(socket_metadata /run/podmesh-manager/control.sock)
existing=$( { systemctl show podmesh.service -p ActiveState -p SubState -p MainPID -p InvocationID -p NRestarts; systemctl show podmesh-web-observer.service -p ActiveState -p SubState -p MainPID -p InvocationID -p NRestarts; } | commit_stdin existing-services)
containers=$(podman ps -a --format json | jq -cS 'map({id:(.Id//.ID),state:(.State//""),started_at:(.StartedAt//""),pid:(.Pid//0),restarts:(.Restarts//0)})|sort_by(.id)' | commit_stdin rootful-podman-containers)
routes=$(observe routes ip -4 route show table all); firewall=$(observe firewall nft list ruleset); listener=$(listeners); inspect=$(inspection); exchange_rows=$(exchanges)   # exchanges() reads what inspection() wrote; the order is required
shutdown=null
if [ -n "$shutdown_report" ]; then
  shutdown=$(jq -ce 'select(.schema_version=="podmesh-manager-graceful-shutdown/v1" and .typed_request_acknowledged==true and .process_exited_successfully==true and .service_inactive==true and .control_socket_absent==true and .forced_signal_used==false)' "$shutdown_report") || { echo 'Graceful shutdown report is invalid' >&2; exit 2; }
fi
jq -nS --arg alias "$alias_name" --arg stage "$stage" --arg version "$version" --arg binary "$binary_hash" --argjson configuration "$config" --argjson dropin "$dropin_value" --argjson service "$service" --argjson process "$process" --argjson state "$state" --argjson runtime "$runtime" --argjson socket "$socket" --argjson listeners "$listener" --arg existing "$existing" --arg containers "$containers" --argjson routes "$routes" --argjson firewall "$firewall" --argjson inspection "$inspect" --argjson exchanges "$exchange_rows" --argjson shutdown "$shutdown" '{schema_version:"podmesh-manager-live-activation-evidence/v2",host_alias:$alias,stage:$stage,package:{name:"podmesh-manager",version:$version,binary_sha256:$binary,dpkg_verify:"clean"},configuration:$configuration,dropin:$dropin,service:$service,manager_process:$process,paths:{state:$state,runtime:$runtime,control_socket:$socket},listeners:$listeners,stability:{existing_services_commitment:$existing,podman_containers_commitment:$containers,routes:$routes,firewall:$firewall},inspection:$inspection,exchanges:$exchanges,graceful_shutdown:$shutdown}' > "$work/evidence.json"
mkdir -p -- "$(dirname -- "$output")"; mv -- "$work/evidence.json" "$output"; sha256sum -- "$output" > "$output.sha256"
