#!/bin/bash
# Bounded configured-manager default-refusal qualification.
#
# This script is intentionally the only mutating command in this harness.  It
# creates one empty private runtime directory for offline validation, removes
# it after proving it is still empty, and asks systemd to start the packaged
# unit while its network mode remains disabled.  It never enables the unit,
# installs a drop-in, changes routing or firewall state, or starts networking.
set -euo pipefail
export LC_ALL=C

usage() {
  echo "Usage: $0 --host-alias <stable-alias> --salt-file <root-0600-salt> --output <evidence.json>" >&2
  exit 2
}

host_alias= salt_file= output=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --host-alias) host_alias=${2-}; shift 2 ;;
    --salt-file) salt_file=${2-}; shift 2 ;;
    --output) output=${2-}; shift 2 ;;
    *) usage ;;
  esac
done

[ "$(id -u)" -eq 0 ] || { echo "Run as root" >&2; exit 2; }
[[ "$host_alias" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$ ]] || usage
[ -n "$output" ] && [ -f "$salt_file" ] || usage
[ "$(stat -c '%a:%u:%F' -- "$salt_file")" = '600:0:regular file' ] || {
  echo "Salt file must be root-owned mode 0600" >&2; exit 2;
}
[ "$(stat -c '%s' -- "$salt_file")" -ge 32 ] || {
  echo "Salt file must contain at least 32 bytes" >&2; exit 2;
}

for command in jq stat sha256sum systemctl journalctl getent install rmdir find wc podman ss ip date readlink awk cut grep tr sleep; do
  command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }
done

path_root=${PODMESH_PATH_ROOT:-}
actual_path() { printf '%s%s' "$path_root" "$1"; }
config_path=/etc/podmesh-manager/config.json
state_path=/var/lib/podmesh-manager
runtime_path=/run/podmesh-manager
binary=${PODMESH_MANAGER_BINARY:-/usr/lib/podmesh-manager/podmesh-managerd}
runuser_bin=${PODMESH_RUNUSER_BIN:-runuser}
work=$(mktemp -d)
created_runtime=0

cleanup() {
  # Never remove a directory that pre-existed this run.  A failed validation is
  # still allowed to leave evidence only when its private directory is empty.
  if [ "$created_runtime" -eq 1 ] && [ -d "$(actual_path "$runtime_path")" ]; then
    if [ "$(find -P "$(actual_path "$runtime_path")" -mindepth 1 -maxdepth 1 -printf . | wc -c | tr -d '[:space:]')" = 0 ]; then
      rmdir -- "$(actual_path "$runtime_path")" || true
    fi
  fi
  rm -rf -- "$work"
}
trap cleanup EXIT

commit_stdin() { { cat -- "$salt_file"; printf '\n'; cat; } | sha256sum | awk '{print "sha256:" $1}'; }
commit_text() { printf '%s' "$1" | commit_stdin; }
capture() {
  local target=$1 value rc
  shift
  set +e
  value=$("$@")
  rc=$?
  set -e
  [ "$rc" -eq 0 ] || { echo "Evidence acquisition failed: $1" >&2; exit 2; }
  printf -v "$target" '%s' "$value"
}

metadata_json() {
  local logical=$1 actual=$2 raw
  if [ -e "$actual" ] || [ -L "$actual" ]; then
    raw=$(stat -c '%F|%u|%g|%a|%s' -- "$actual") || return 1
    jq -Rn --arg path "$logical" --arg raw "$raw" '
      ($raw|split("|")) as $v |
      if ($v|length)!=5 or ($v[1]|test("^[0-9]+$")|not) or ($v[2]|test("^[0-9]+$")|not) or ($v[3]|test("^[0-7]{3,4}$")|not) or ($v[4]|test("^[0-9]+$")|not)
      then error("invalid metadata")
      else {path:$path,present:true,file_type:$v[0],uid:($v[1]|tonumber),gid:($v[2]|tonumber),mode:$v[3],bytes:($v[4]|tonumber)} end'
  else
    jq -cn --arg path "$logical" '{path:$path,present:false,file_type:null,uid:null,gid:null,mode:null,bytes:null}'
  fi
}

entry_count() { find -P "$1" -mindepth 1 -maxdepth 1 -printf . | wc -c | tr -d '[:space:]'; }

manager_process_count() {
  local entry pid exe cmd_bytes count=0 proc_root=${PODMESH_PROC_ROOT:-/proc}
  for entry in "$proc_root"/[0-9]*; do
    [ -d "$entry" ] || continue
    pid=${entry#"$proc_root"/}
    if ! exe=$(readlink -- "$entry/exe" 2>/dev/null); then
      [ ! -d "$entry" ] && continue
      cmd_bytes=$(wc -c < "$entry/cmdline" 2>/dev/null) || { echo "Cannot inspect process $pid" >&2; return 1; }
      [ "$cmd_bytes" = 0 ] && continue
      echo "Cannot resolve executable for live process $pid" >&2; return 1
    fi
    exe=${exe% (deleted)}
    [ "$exe" = /usr/lib/podmesh-manager/podmesh-managerd ] && count=$((count + 1))
  done
  printf '%s\n' "$count"
}

unit_raw() {
  systemctl show podmesh-manager.service --no-page \
    --property=LoadState --property=ActiveState --property=SubState --property=UnitFileState \
    --property=MainPID --property=ExecMainPID --property=ExecMainStatus --property=Result \
    --property=InvocationID --property=ExecMainStartTimestampMonotonic \
    --property=NRestarts --property=FragmentPath --property=DropInPaths
}

unit_json() {
  local raw=$1 fragment logical actual dropins
  fragment=$(jq -rRn --arg raw "$raw" 'reduce ($raw|split("\n")[]|select(length>0)|split("=")|{k:.[0],v:(.[1:]|join("="))}) as $r ({};.[$r.k]=$r.v) | .FragmentPath // empty')
  [ -n "$fragment" ] || { echo "systemd did not report FragmentPath" >&2; return 1; }
  actual=$(actual_path "$fragment")
  [ -f "$actual" ] && [ ! -L "$actual" ] || { echo "Unit fragment is not a regular file" >&2; return 1; }
  dropins=$(jq -Rn --arg raw "$raw" '
    reduce ($raw|split("\n")[]|select(length>0)|split("=")|{k:.[0],v:(.[1:]|join("="))}) as $r ({};.[$r.k]=$r.v) |
    (.DropInPaths // "") | if length==0 then [] else split(" ") | map(select(length>0)) end')
  jq -e -n --arg raw "$raw" --arg fragment "$fragment" --arg fragment_sha256 "$(sha256sum -- "$actual"|awk '{print $1}')" --argjson dropins "$dropins" '
    reduce ($raw|split("\n")[]|select(length>0)|split("=")|{k:.[0],v:(.[1:]|join("="))}) as $r
      ({load_state:null,active_state:null,sub_state:null,unit_file_state:null,main_pid:null,exec_main_pid:null,exec_main_status:null,result:null,invocation_id:null,start_monotonic_usec:null,n_restarts:null};
       if $r.k=="LoadState" then .load_state=$r.v
       elif $r.k=="ActiveState" then .active_state=$r.v
       elif $r.k=="SubState" then .sub_state=$r.v
       elif $r.k=="UnitFileState" then .unit_file_state=$r.v
       elif $r.k=="MainPID" then .main_pid=($r.v|tonumber)
       elif $r.k=="ExecMainPID" then .exec_main_pid=($r.v|tonumber)
       elif $r.k=="ExecMainStatus" then .exec_main_status=($r.v|tonumber)
       elif $r.k=="Result" then .result=$r.v
       elif $r.k=="InvocationID" then .invocation_id=($r.v|if length==0 then null else . end)
       elif $r.k=="ExecMainStartTimestampMonotonic" then .start_monotonic_usec=($r.v|tonumber)
       elif $r.k=="NRestarts" then .n_restarts=($r.v|tonumber) else . end) |
    if ([.load_state,.active_state,.sub_state,.unit_file_state,.main_pid,.exec_main_pid,.exec_main_status,.result,.start_monotonic_usec,.n_restarts] | any(.==null)) then error("incomplete systemd unit reading") else . end |
    . + {fragment_path:$fragment,fragment_sha256:$fragment_sha256,dropins:$dropins}'
}

wait_for_default_refusal() {
  # Type=simple can return from `systemctl start` while the manager process is
  # still transitioning.  Bind the later journal and state capture to a bounded
  # terminal observation instead of treating that command's exit status as the
  # proof.
  local deadline=$((SECONDS + 5)) raw unit
  while :; do
    raw=$(unit_raw) || return 1
    unit=$(unit_json "$raw") || return 1
  if jq -e '.load_state=="loaded" and .active_state=="failed" and .sub_state=="failed" and .unit_file_state=="disabled" and .main_pid==0 and .exec_main_pid>0 and .exec_main_status==1 and .result=="exit-code" and .n_restarts==0 and (.dropins|length)==0' <<<"$unit" >/dev/null; then
      printf '%s\n' "$unit"
      return 0
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "Timed out waiting for the required default-network refusal state" >&2
      return 1
    fi
    sleep 0.1
  done
}

dropin_hashes_json() {
  local path actual sha list='[]'
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    actual=$(actual_path "$path")
    [ -f "$actual" ] && [ ! -L "$actual" ] || { echo "systemd drop-in is not a regular file" >&2; return 1; }
    sha=$(sha256sum -- "$actual" | awk '{print $1}') || return 1
    list=$(jq -c --arg path_commitment "$(commit_text "$path")" --arg sha256 "$sha" '. + [{path_commitment:$path_commitment,sha256:$sha256}]' <<<"$list") || return 1
  done < <(jq -r '.[]' <<<"$1")
  jq -cS . <<<"$list"
}

existing_service_commitment() {
  local unit=$1 raw semantic
  raw=$(systemctl show "$unit" --no-page --property=LoadState --property=ActiveState --property=SubState --property=UnitFileState --property=MainPID --property=ExecMainPID --property=InvocationID --property=ExecMainStartTimestampMonotonic --property=NRestarts) || return 1
  semantic=$(jq -e -Rn --arg raw "$raw" '
    reduce ($raw|split("\n")[]|select(length>0)|split("=")|{k:.[0],v:(.[1:]|join("="))}) as $r
    ({load_state:null,active_state:null,sub_state:null,unit_file_state:null,main_pid:null,exec_main_pid:null,invocation_id:null,start_monotonic_usec:null,n_restarts:null};
      if $r.k=="LoadState" then .load_state=$r.v elif $r.k=="ActiveState" then .active_state=$r.v elif $r.k=="SubState" then .sub_state=$r.v elif $r.k=="UnitFileState" then .unit_file_state=$r.v elif $r.k=="MainPID" then .main_pid=($r.v|tonumber) elif $r.k=="ExecMainPID" then .exec_main_pid=($r.v|tonumber) elif $r.k=="InvocationID" then .invocation_id=($r.v|if length==0 then null else . end) elif $r.k=="ExecMainStartTimestampMonotonic" then .start_monotonic_usec=($r.v|tonumber) elif $r.k=="NRestarts" then .n_restarts=($r.v|tonumber) else . end) |
      if ([.load_state,.active_state,.sub_state,.unit_file_state,.main_pid,.exec_main_pid,.start_monotonic_usec,.n_restarts]|any(.==null)) then error("incomplete existing service reading") else . end') || return 1
  jq -cn --arg unit "$unit" --arg commitment "$(printf '%s' "$semantic" | commit_stdin)" '{unit:$unit,commitment:$commitment}'
}

podman_containers_commitment() {
  local ids id inspect projected manifest="$work/containers.json"
  printf '[]\n' > "$manifest"
  ids=$(podman container ls -aq) || return 1
  while IFS= read -r id; do
    [ -n "$id" ] || continue
    inspect=$(podman inspect --type container --format '{{json .}}' "$id") || return 1
    projected=$(jq -e -cS '{id:(.Id//error("missing id")),state:(.State.Status//error("missing state")),started_at:(.State.StartedAt//error("missing start timestamp")),pid:(.State.Pid//error("missing pid")),restarts:(.RestartCount//error("missing restart count"))}' <<<"$inspect") || return 1
    jq --argjson projected "$projected" '. + [$projected]' "$manifest" > "$manifest.next" || return 1
    mv -- "$manifest.next" "$manifest"
  done <<<"$ids"
  jq -cS 'sort_by(.id)' "$manifest" | commit_stdin
}

observation_json() {
  local command_name=$1 command_path=$2 output rc bytes commitment
  if ! command -v "$command_path" >/dev/null; then
    jq -cn --arg reason "$command_name-unavailable" '{status:"unknown",reason:$reason,bytes:null,commitment:null}'
    return
  fi
  set +e
  output=$($command_path "${@:3}" 2>"$work/$command_name.err")
  rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then
    jq -cn --arg reason "$command_name-command-failed" '{status:"unknown",reason:$reason,bytes:null,commitment:null}'
    return
  fi
  bytes=$(printf '%s' "$output" | wc -c | tr -d '[:space:]')
  commitment=$(printf '%s' "$output" | commit_stdin)
  jq -cn --argjson bytes "$bytes" --arg commitment "$commitment" '{status:"available-successful",reason:null,bytes:$bytes,commitment:$commitment}'
}

listener_json() {
  local endpoint=$1 port raw tcp udp
  port=${endpoint##*:}
  [[ "$port" =~ ^[0-9]+$ ]] || { echo "Invalid bind port" >&2; return 1; }
  raw=$(ss -H -ltnup) || { echo "ss listener observation failed" >&2; return 1; }
  tcp=$(awk -v port="$port" '$1 ~ /^tcp/ {x=$5; sub(/^.*:/,"",x); if (x==port) c++} END{print c+0}' <<<"$raw")
  udp=$(awk -v port="$port" '$1 ~ /^udp/ {x=$5; sub(/^.*:/,"",x); if (x==port) c++} END{print c+0}' <<<"$raw")
  jq -cn --arg endpoint_commitment "$(commit_text "$endpoint")" --argjson tcp "$tcp" --argjson udp "$udp" --arg snapshot_commitment "$(printf '%s' "$raw"|commit_stdin)" '{status:"available-successful",endpoint_commitment:$endpoint_commitment,tcp_listener_count:$tcp,udp_listener_count:$udp,snapshot_commitment:$snapshot_commitment}'
}

manager_configuration_json() {
  local actual=$1 normalized logical replica host topology peers config_commitment observation_writer_uid grant_count
  normalized=$(jq -e -cS '
    . as $root | .network as $network |
    ($network.manager.replicas | sort_by(.replica_id, .host_id)) as $replicas |
    ($network.manager.grants | sort_by(.scope, .owner_replica_id)) as $grants |
    ($replicas[] | select(.replica_id == $network.replica_id) | .host_id) as $host |
    if ($host|type)!="string" then error("local replica is absent from topology")
    elif ($root.observation_writer_uid|type)!="number" or $root.observation_writer_uid != ($root.observation_writer_uid|floor) or $root.observation_writer_uid < 0 then error("observation_writer_uid must be a non-negative integer")
    else
    {logical_manager_id:$network.manager.logical_manager_id,local_replica_id:$network.replica_id,local_host_id:$host,observation_writer_uid:$root.observation_writer_uid,topology:{replicas:$replicas,grants:$grants},peers:($network.peers|sort_by(.replica_id)|map({replica_id,endpoint,shared_key_hex}))}
    end' "$actual") || return 1
  logical=$(jq -r .logical_manager_id <<<"$normalized")
  replica=$(jq -r .local_replica_id <<<"$normalized")
  host=$(jq -r .local_host_id <<<"$normalized")
  observation_writer_uid=$(jq -r .observation_writer_uid <<<"$normalized")
  topology=$(jq -cS .topology <<<"$normalized")
  grant_count=$(jq -r '.topology.grants | length' <<<"$normalized")
  peers=$(jq -cS '[.peers[] | {replica_id_commitment:null,endpoint_commitment:null,shared_key_commitment:null}]' <<<"$normalized")
  # Replace each placeholder from the original ordered entries.  All values,
  # including UUIDs and pair keys, remain salted commitments in the evidence.
  while IFS=$'\t' read -r position peer_id endpoint shared_key; do
    peers=$(jq -c --argjson position "$position" --arg replica "$(commit_text "$peer_id")" --arg endpoint "$(commit_text "$endpoint")" --arg key "$(commit_text "$shared_key")" '.[$position]={replica_id_commitment:$replica,endpoint_commitment:$endpoint,shared_key_commitment:$key}' <<<"$peers") || return 1
  done < <(jq -r '.peers | to_entries[] | [.key,.value.replica_id,.value.endpoint,.value.shared_key_hex] | @tsv' <<<"$normalized")
  config_commitment=$(cat -- "$actual" | commit_stdin)
  jq -cn --arg document_commitment "$config_commitment" --arg logical_manager_commitment "$(commit_text "$logical")" --arg local_replica_commitment "$(commit_text "$replica")" --arg local_host_commitment "$(commit_text "$host")" --argjson observation_writer_uid "$observation_writer_uid" --argjson grant_count "$grant_count" --arg topology_commitment "$(printf '%s' "$topology"|commit_stdin)" --argjson peers "$peers" '{document_commitment:$document_commitment,logical_manager_commitment:$logical_manager_commitment,local_replica_commitment:$local_replica_commitment,local_host_commitment:$local_host_commitment,observation_writer_uid:$observation_writer_uid,grant_count:$grant_count,topology_commitment:$topology_commitment,peer_count:($peers|length),peers:$peers}'
}

actual_config=$(actual_path "$config_path")
actual_state=$(actual_path "$state_path")
actual_runtime=$(actual_path "$runtime_path")
[ -f "$actual_config" ] && [ ! -L "$actual_config" ] || { echo "Protected manager config must be a regular file" >&2; exit 2; }
[ -d "$actual_state" ] && [ ! -L "$actual_state" ] || { echo "Manager state directory must already exist" >&2; exit 2; }
[ ! -e "$actual_runtime" ] && [ ! -L "$actual_runtime" ] || { echo "Refusing to reuse an existing manager runtime directory" >&2; exit 2; }

account=$(getent passwd podmesh-manager) || { echo "Manager account missing" >&2; exit 2; }
group=$(getent group podmesh-manager) || { echo "Manager group missing" >&2; exit 2; }
manager_uid=$(cut -d: -f3 <<<"$account")
manager_gid=$(cut -d: -f3 <<<"$group")
[[ "$manager_uid" =~ ^[1-9][0-9]*$ && "$manager_gid" =~ ^[1-9][0-9]*$ ]] || { echo "Invalid manager account identity" >&2; exit 2; }
config_metadata=$(metadata_json "$config_path" "$actual_config")
jq -e --argjson uid "$manager_uid" --argjson gid "$manager_gid" '.present and .file_type=="regular file" and .uid==0 and .gid==$gid and .mode=="640"' <<<"$config_metadata" >/dev/null || {
  echo "Manager config must be root-owned, group-readable by podmesh-manager, mode 0640" >&2; exit 2;
}
manager_config=$(manager_configuration_json "$actual_config")
bind_endpoint=$(jq -r '.network.bind' "$actual_config")
control_socket=$(jq -r '.control_socket' "$actual_config")
[[ "$control_socket" == "$runtime_path"/* ]] || { echo "Control socket must reside in the private runtime directory" >&2; exit 2; }

capture pre_unit_raw unit_raw
pre_unit=$(unit_json "$pre_unit_raw")
pre_dropins=$(dropin_hashes_json "$(jq -c .dropins <<<"$pre_unit")")
pre_unit=$(jq --argjson dropins "$pre_dropins" '.dropins=$dropins | .invocation_id=if .invocation_id==null then null else "redacted" end' <<<"$pre_unit")
jq -e '.load_state=="loaded" and .active_state=="inactive" and .sub_state=="dead" and .unit_file_state=="disabled" and .main_pid==0 and .exec_main_pid==0 and .invocation_id==null and .n_restarts==0 and (.dropins|length)==0' <<<"$pre_unit" >/dev/null || {
  echo "Manager unit is not in the required default-disabled precondition" >&2; exit 2;
}
pre_existing=$(jq -cn --argjson lifecycle "$(existing_service_commitment podmesh.service)" --argjson observer "$(existing_service_commitment podmesh-web-observer.service)" '{lifecycle:$lifecycle,observer:$observer}')
pre_podman=$(podman_containers_commitment)
pre_routes=$(observation_json routes ip -j route show table all)
pre_firewall=$(observation_json firewall nft list ruleset)

install -d -o "$manager_uid" -g "$manager_gid" -m 0700 -- "$actual_runtime"
created_runtime=1
runtime_created=$(metadata_json "$runtime_path" "$actual_runtime")
runtime_initial_entries=$(entry_count "$actual_runtime")
[ "$runtime_initial_entries" = 0 ] || { echo "New runtime directory is not empty" >&2; exit 2; }
validation_uid=$($runuser_bin -u podmesh-manager -- id -u) || { echo "Cannot execute as manager account" >&2; exit 2; }
[ "$validation_uid" = "$manager_uid" ] || { echo "Validation effective UID differs from manager account" >&2; exit 2; }
set +e
validation_output=$($runuser_bin -u podmesh-manager -- "$binary" --config "$config_path" --state-dir "$state_path" --runtime-dir "$runtime_path" --validate-config 2>"$work/validation.err")
validation_rc=$?
set -e
[ "$validation_rc" -eq 0 ] || { echo "Offline config validation failed" >&2; exit 2; }
printf '%s' "$validation_output" | jq -e '.configuration_valid==true and .durable_store_checked==false and .network_started==false' >/dev/null || {
  echo "Offline validation returned an unexpected result" >&2; exit 2;
}
runtime_after_validation_entries=$(entry_count "$actual_runtime")
[ "$runtime_after_validation_entries" = 0 ] || { echo "Offline validation created runtime artifacts" >&2; exit 2; }
runtime_validation_metadata=$(metadata_json "$runtime_path" "$actual_runtime")
rmdir -- "$actual_runtime"
created_runtime=0
[ ! -e "$actual_runtime" ] && [ ! -L "$actual_runtime" ] || { echo "Validation runtime directory was not removed" >&2; exit 2; }

set +e
systemctl start podmesh-manager.service >"$work/start.out" 2>"$work/start.err"
start_rc=$?
set -e
post_unit_full=$(wait_for_default_refusal) || exit 2
post_invocation=$(jq -r '.invocation_id // empty' <<<"$post_unit_full")
[ -n "$post_invocation" ] || { echo "Refusal attempt has no systemd InvocationID" >&2; exit 2; }
post_dropins=$(dropin_hashes_json "$(jq -c .dropins <<<"$post_unit_full")")
post_unit=$(jq --argjson dropins "$post_dropins" --arg invocation_commitment "$(commit_text "$post_invocation")" '.dropins=$dropins | .invocation_id=$invocation_commitment' <<<"$post_unit_full")
journal=$(journalctl --no-pager --output=short-iso-precise "_SYSTEMD_INVOCATION_ID=$post_invocation") || { echo "Invocation-bound journal acquisition failed" >&2; exit 2; }
[ -n "$journal" ] || { echo "Invocation-bound journal is empty" >&2; exit 2; }
grep -Fq 'runtime networking disabled; set PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers explicitly' <<<"$journal" || {
  echo "Invocation-bound journal does not prove the network-disabled refusal" >&2; exit 2;
}
journal_json=$(jq -cn --arg invocation_commitment "$(commit_text "$post_invocation")" --arg transcript_commitment "$(printf '%s' "$journal"|commit_stdin)" --argjson line_count "$(wc -l <<<"$journal"|tr -d '[:space:]')" '{selector:"_SYSTEMD_INVOCATION_ID",invocation_commitment:$invocation_commitment,transcript_commitment:$transcript_commitment,line_count:$line_count,network_disabled_refusal_observed:true}')

post_existing=$(jq -cn --argjson lifecycle "$(existing_service_commitment podmesh.service)" --argjson observer "$(existing_service_commitment podmesh-web-observer.service)" '{lifecycle:$lifecycle,observer:$observer}')
post_podman=$(podman_containers_commitment)
post_routes=$(observation_json routes ip -j route show table all)
post_firewall=$(observation_json firewall nft list ruleset)
process_count=$(manager_process_count)
state_metadata=$(metadata_json "$state_path" "$actual_state")
state_entries=$(entry_count "$actual_state")
socket_metadata=$(metadata_json "$control_socket" "$(actual_path "$control_socket")")
listeners=$(listener_json "$bind_endpoint")

existing_unchanged=$([ "$pre_existing" = "$post_existing" ] && printf true || printf false)
podman_unchanged=$([ "$pre_podman" = "$post_podman" ] && printf true || printf false)
routes_unchanged=$(jq -n --argjson before "$pre_routes" --argjson after "$post_routes" 'if $before.status=="available-successful" and $after.status=="available-successful" then $before.commitment==$after.commitment else null end')
firewall_unchanged=$(jq -n --argjson before "$pre_firewall" --argjson after "$post_firewall" 'if $before.status=="available-successful" and $after.status=="available-successful" then $before.commitment==$after.commitment else null end')
[ "$existing_unchanged" = true ] || { echo "Existing PodMesh service identity/PID/start/restart changed" >&2; exit 2; }
[ "$podman_unchanged" = true ] || { echo "Rootful Podman container identity/PID/start/restart changed" >&2; exit 2; }
[ "$routes_unchanged" != false ] || { echo "IPv4 routes changed during the refusal window" >&2; exit 2; }
[ "$firewall_unchanged" != false ] || { echo "nftables ruleset changed during the refusal window" >&2; exit 2; }
[ "$process_count" = 0 ] || { echo "Manager process remains after refusal" >&2; exit 2; }
jq -e --argjson entries "$state_entries" '.present and .file_type=="directory" and .entry_count==$entries' < <(jq --argjson entries "$state_entries" '. + {entry_count:$entries}' <<<"$state_metadata") >/dev/null || { echo "Cannot record manager state directory" >&2; exit 2; }
[ "$state_entries" = 0 ] || { echo "Manager state directory contains artifacts after refusal" >&2; exit 2; }
jq -e '.present == false' <<<"$socket_metadata" >/dev/null || { echo "Manager control socket remains after refusal" >&2; exit 2; }
jq -e '.tcp_listener_count==0 and .udp_listener_count==0' <<<"$listeners" >/dev/null || { echo "Configured manager endpoint still listens after refusal" >&2; exit 2; }

mkdir -p -- "$(dirname -- "$output")"
jq -n \
  --arg schema_version 'podmesh-manager-default-refusal-evidence/v1' \
  --arg host_alias "$host_alias" \
  --arg captured_at_utc "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson config_metadata "$config_metadata" \
  --argjson configuration "$manager_config" \
  --argjson pre_unit "$pre_unit" \
  --argjson post_unit "$post_unit" \
  --argjson validation_uid "$validation_uid" --argjson account_uid "$manager_uid" --argjson runtime_validation_metadata "$runtime_validation_metadata" \
  --argjson journal "$journal_json" \
  --argjson start_exit_status "$start_rc" \
  --argjson before_existing "$pre_existing" --argjson after_existing "$post_existing" \
  --arg before_podman "$pre_podman" --arg after_podman "$post_podman" \
  --argjson before_routes "$pre_routes" --argjson after_routes "$post_routes" \
  --argjson before_firewall "$pre_firewall" --argjson after_firewall "$post_firewall" \
  --argjson process_count "$process_count" --argjson state_metadata "$state_metadata" --argjson state_entries "$state_entries" \
  --argjson socket_metadata "$socket_metadata" --argjson listeners "$listeners" \
  --argjson existing_unchanged "$existing_unchanged" --argjson podman_unchanged "$podman_unchanged" \
  --argjson routes_unchanged "$routes_unchanged" --argjson firewall_unchanged "$firewall_unchanged" \
  '{schema_version:$schema_version,host_alias:$host_alias,captured_at_utc:$captured_at_utc,configuration:{metadata:$config_metadata,commitments:$configuration},offline_validation:{command:["podmesh-managerd","--config","/etc/podmesh-manager/config.json","--state-dir","/var/lib/podmesh-manager","--runtime-dir","/run/podmesh-manager","--validate-config"],effective_uid:$validation_uid,account_uid:$account_uid,configuration_valid:true,runtime:{created:true,metadata:$runtime_validation_metadata,initial_entry_count:0,post_validation_entry_count:0,removed:true}},default_refusal:{start_exit_status:$start_exit_status,pre_unit:$pre_unit,post_unit:$post_unit,journal:$journal},before:{existing_services:$before_existing,podman_containers_commitment:$before_podman,routes:$before_routes,firewall:$before_firewall},after:{existing_services:$after_existing,podman_containers_commitment:$after_podman,routes:$after_routes,firewall:$after_firewall,manager:{process_count:$process_count,state_directory:($state_metadata+{entry_count:$state_entries}),control_socket:$socket_metadata,listeners:$listeners}},assertions:{existing_services_unchanged:$existing_unchanged,podman_containers_unchanged:$podman_unchanged,routes_unchanged:$routes_unchanged,firewall_unchanged:$firewall_unchanged,no_manager_process:($process_count==0),state_directory_empty:($state_entries==0),control_socket_absent:($socket_metadata.present|not),configured_endpoint_not_listening:(($listeners.tcp_listener_count==0) and ($listeners.udp_listener_count==0))}}' | jq -S . > "$output"
sha256sum -- "$output" > "$output.sha256"
