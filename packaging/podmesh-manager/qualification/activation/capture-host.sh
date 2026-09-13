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

commit_stdin() { { cat -- "$salt"; printf '\000%s\000' "$1"; cat; } | sha256sum | awk '{print "sha256:" $1}'; }
commit_file() { { cat -- "$salt"; printf '\000%s\000' "$1"; cat -- "$2"; } | sha256sum | awk '{print "sha256:" $1}'; }
commit_text() { printf '%s' "$2" | commit_stdin "$1"; }
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
    peers=$(jq -c --arg replica "$(commit_text peer-replica "$replica")" --arg endpoint "$(commit_text peer-endpoint "$endpoint")" --arg key "$(commit_text peer-key "$key")" '.+[{replica_id_commitment:$replica,endpoint_commitment:$endpoint,shared_key_commitment:$key}]' <<<"$peers")
  done < <(jq -r '.peers[]|[.replica_id,.endpoint,.shared_key_hex]|@tsv' <<<"$normalized")
  topology=$(jq -cS .topology <<<"$normalized")
  jq -cn --arg document "$(commit_file config-document "$path")" --arg logical "$(commit_text logical-manager "$(jq -r .logical <<<"$normalized")")" --arg replica "$(commit_text local-replica "$(jq -r .replica <<<"$normalized")")" --arg host "$(commit_text local-host "$(jq -r .host <<<"$normalized")")" --arg topology "$(printf '%s' "$topology" | commit_stdin topology)" --argjson peers "$peers" '{document_commitment:$document,logical_manager_commitment:$logical,local_replica_commitment:$replica,local_host_commitment:$host,topology_commitment:$topology,peer_count:($peers|length),peers:$peers}'
}
dropin() {
  local installed=/etc/systemd/system/podmesh-manager.service.d/90-g2-network.conf packaged=/usr/lib/systemd/system/podmesh-manager.service paths
  [ -f "$packaged" ] && grep -Fxq 'IPAddressDeny=any' "$packaged" || { echo 'Packaged deny-all policy is absent' >&2; return 1; }
  paths=$(systemctl show podmesh-manager.service --no-page -p DropInPaths --value) || return 1
  if [ -n "$paths" ] && [ "$paths" != "$installed" ]; then echo 'Undeclared manager systemd drop-in is present' >&2; return 1; fi
  if [ -e "$installed" ]; then
    [ -f "$installed" ] && [ ! -L "$installed" ] || { echo 'Activation drop-in is unsafe' >&2; return 1; }
    local limits environment families allows denies expected_allow actual_allow policy
    limits=$(python3 "$root/validate-dropin.py" --dropin "$installed") || return 1
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
    jq -cn --arg sha "$(sha256sum -- "$installed"|awk '{print $1}')" --arg fragment "$(sha256sum -- "$packaged"|awk '{print $1}')" --arg policy "$policy" --argjson limits "$limits" '{present:true,sha256:$sha,semantic_limits:$limits,packaged_fragment_sha256:$fragment,inherited_deny_all:true,effective_policy_configured:true,effective_policy_commitment:$policy}'
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
  jq -cn --arg endpoint "$(commit_text bind-endpoint "$endpoint")" --argjson tcp "$tcp" --argjson udp "$udp" '{status:"available-successful",endpoint_commitment:$endpoint,tcp_listener_count:$tcp,udp_listener_count:$udp}'
}
inspection() {
  [ "$with_inspection" -eq 1 ] || { printf '%s\n' null; return; }
  local raw; raw=$(runuser -u podmesh-manager -- /usr/lib/podmesh-manager/podmesh-managerd --inspect-store --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager) || { echo 'Read-only canonical inspection failed' >&2; return 1; }
  jq -ce --arg logical "$(commit_text inspection-logical "$(jq -r .logical_manager_id <<<"$raw")")" --arg replica "$(commit_text inspection-replica "$(jq -r .replica_id <<<"$raw")")" '{schema_version,logical_manager_commitment:$logical,replica_commitment:$replica,logical_history_sha256,sqlite_integrity_result,history_count,receipt_count,audit_event_count,incomplete_attempt_count:(.incomplete_attempts|length)}' <<<"$raw"
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
routes=$(observe routes ip -4 route show table all); firewall=$(observe firewall nft list ruleset); listener=$(listeners); inspect=$(inspection)
shutdown=null
if [ -n "$shutdown_report" ]; then
  shutdown=$(jq -ce 'select(.schema_version=="podmesh-manager-graceful-shutdown/v1" and .typed_request_acknowledged==true and .process_exited_successfully==true and .service_inactive==true and .control_socket_absent==true and .forced_signal_used==false)' "$shutdown_report") || { echo 'Graceful shutdown report is invalid' >&2; exit 2; }
fi
jq -nS --arg alias "$alias_name" --arg stage "$stage" --arg version "$version" --arg binary "$binary_hash" --argjson configuration "$config" --argjson dropin "$dropin_value" --argjson service "$service" --argjson process "$process" --argjson state "$state" --argjson runtime "$runtime" --argjson socket "$socket" --argjson listeners "$listener" --arg existing "$existing" --arg containers "$containers" --argjson routes "$routes" --argjson firewall "$firewall" --argjson inspection "$inspect" --argjson shutdown "$shutdown" '{schema_version:"podmesh-manager-live-activation-evidence/v2",host_alias:$alias,stage:$stage,package:{name:"podmesh-manager",version:$version,binary_sha256:$binary,dpkg_verify:"clean"},configuration:$configuration,dropin:$dropin,service:$service,manager_process:$process,paths:{state:$state,runtime:$runtime,control_socket:$socket},listeners:$listeners,stability:{existing_services_commitment:$existing,podman_containers_commitment:$containers,routes:$routes,firewall:$firewall},inspection:$inspection,graceful_shutdown:$shutdown}' > "$work/evidence.json"
mkdir -p -- "$(dirname -- "$output")"; mv -- "$work/evidence.json" "$output"; sha256sum -- "$output" > "$output.sha256"
