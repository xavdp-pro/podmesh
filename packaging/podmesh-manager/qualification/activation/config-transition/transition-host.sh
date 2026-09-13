#!/bin/bash
# Bounded and resumable exact local configuration transitions: zero grants to the three reviewed grants
# before the first store open, and incoming_workers from exactly 1 to exactly 2 after activation.
set -euo pipefail
export LC_ALL=C
umask 077

root=$(cd -- "$(dirname -- "$0")" && pwd)
usage() {
  echo "Usage: $0 --mode apply|rollback|seal-for-activation --host-alias lab-a|lab-b|lab-c --mapping <private-map.json> --salt-file <private-salt> --candidate-verification <report.json> --backup-file </root/file> --evidence-directory <private-directory> [--transition three-grants|incoming-workers]" >&2
  exit 2
}
mode= alias_name= mapping= salt= report= backup= evidence= transition=three-grants
while [ "$#" -gt 0 ]; do
  case "$1" in
    --mode) mode=${2-}; shift 2;;
    --host-alias) alias_name=${2-}; shift 2;;
    --mapping) mapping=${2-}; shift 2;;
    --salt-file) salt=${2-}; shift 2;;
    --candidate-verification) report=${2-}; shift 2;;
    --backup-file) backup=${2-}; shift 2;;
    --evidence-directory) evidence=${2-}; shift 2;;
    --transition) transition=${2-}; shift 2;;
    *) usage;;
  esac
done
case "$mode:$alias_name" in apply:lab-a|apply:lab-b|apply:lab-c|rollback:lab-a|rollback:lab-b|rollback:lab-c|seal-for-activation:lab-a|seal-for-activation:lab-b|seal-for-activation:lab-c) ;; *) usage;; esac
case "$transition" in three-grants|incoming-workers) ;; *) usage;; esac
[ "$(id -u)" -eq 0 ] && [ -n "$mapping" ] && [ -n "$salt" ] && [ -n "$report" ] && [ -n "$backup" ] && [ -n "$evidence" ] || usage
case "$backup" in /root/*/*|/root/) echo 'Backup must be a direct regular file below /root' >&2; exit 2;; /root/*) ;; *) echo 'Backup must be a direct regular file below /root' >&2; exit 2;; esac
case "$mapping:$salt:$report:$evidence" in /root/*:/root/*:/root/*:/root/*) ;; *) echo 'Mapping, salt, candidate report and evidence directory must be below /root' >&2; exit 2;; esac
for command in python3 jq systemctl runuser getent stat find install mktemp mv sha256sum ss readlink awk cat cut chmod chown rmdir flock dpkg dpkg-query; do
  command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }
done
lock_dir=/run/podmesh-manager-qualification
if [ ! -e "$lock_dir" ] && [ ! -L "$lock_dir" ]; then install -d -m 0700 -o root -g root -- "$lock_dir"; fi
[ -d "$lock_dir" ] && [ ! -L "$lock_dir" ] && [ "$(stat -c '%a:%u:%g' -- "$lock_dir")" = 700:0:0 ] || { echo 'Transition lock directory is unsafe' >&2; exit 2; }
exec 9>"$lock_dir/manager2.lock"
[ "$(stat -c '%F:%a:%u:%g:%h' -- "$lock_dir/manager2.lock")" = 'regular empty file:600:0:0:1' ] || { echo 'Shared manager2 lock file is unsafe' >&2; exit 2; }
flock -n 9 || { echo 'Another manager configuration transition is running' >&2; exit 1; }

config=/etc/podmesh-manager/config.json
state=/var/lib/podmesh-manager
runtime=/run/podmesh-manager
socket=$runtime/control.sock
binary=/usr/lib/podmesh-manager/podmesh-managerd
activation_marker=/etc/podmesh-manager/.manager2-activation-started
ledger=$evidence/config-transition-ledger.json
result=$evidence/config-transition-result.json

regular_protected() {
  local path=$1 mode_expected=$2 uid_expected=$3 gid_expected=$4
  [ -f "$path" ] && [ ! -L "$path" ] &&
    [ "$(stat -c '%F:%a:%u:%g:%h' -- "$path")" = "regular file:$mode_expected:$uid_expected:$gid_expected:1" ]
}
manager_uid=$(getent passwd podmesh-manager | cut -d: -f3)
manager_gid=$(getent group podmesh-manager | cut -d: -f3)
[[ "$manager_uid" =~ ^[1-9][0-9]*$ && "$manager_gid" =~ ^[1-9][0-9]*$ ]] || { echo 'podmesh-manager account is unavailable or privileged' >&2; exit 2; }
regular_protected "$config" 640 0 "$manager_gid" || { echo 'Configuration metadata is unsafe' >&2; exit 2; }
regular_protected "$mapping" 600 0 0 || { echo 'Mapping must be a root-owned mode 0600 regular file with one link' >&2; exit 2; }
regular_protected "$salt" 600 0 0 && [ "$(stat -c %s -- "$salt")" -ge 32 ] || { echo 'Salt must be root-owned mode 0600 with at least 32 bytes' >&2; exit 2; }
regular_protected "$report" 600 0 0 || { echo 'Candidate report must be a root-owned mode 0600 regular file with one link' >&2; exit 2; }
[ -x "$binary" ] && [ ! -L "$binary" ] || { echo 'Packaged manager binary is unavailable or symlinked' >&2; exit 2; }
package_version=$(jq -er 'select(.schema_version=="podmesh-manager-candidate-verification/v2" and .package=="podmesh-manager")|.version|select(type=="string" and length>0)' "$report") || { echo 'Candidate verification report is invalid' >&2; exit 2; }
binary_hash=$(jq -er '.binary_sha256|select(type=="string" and test("^[0-9a-f]{64}$"))' "$report") || { echo 'Candidate binary commitment is invalid' >&2; exit 2; }
[ "$(dpkg-query -W -f='${db:Status-Status}' podmesh-manager)" = installed ] && [ "$(dpkg-query -W -f='${Version}' podmesh-manager)" = "$package_version" ] || { echo 'Installed package differs from the qualified candidate' >&2; exit 1; }
[ "$(sha256sum -- "$binary" | awk '{print $1}')" = "$binary_hash" ] || { echo 'Installed binary differs from the qualified candidate' >&2; exit 1; }
set +e
dpkg_verify=$(dpkg --verify podmesh-manager 2>&1); dpkg_verify_rc=$?
set -e
[ "$dpkg_verify_rc" -eq 0 ] && [ -z "$dpkg_verify" ] || { echo 'Installed package verification is not clean' >&2; exit 1; }
[ -d "$state" ] && [ ! -L "$state" ] && [ "$(stat -c '%a:%u:%g' -- "$state")" = "750:$manager_uid:$manager_gid" ] || { echo 'State directory metadata is unsafe' >&2; exit 2; }
[ ! -e "$socket" ] && [ ! -L "$socket" ] || { echo 'Manager control socket exists' >&2; exit 1; }
durable_state_present=false; activation_marker_present=false
if [ "$transition" = three-grants ]; then
  # The first store open binds the grants; this transition exists only before it, while nothing durable exists.
  [ -z "$(find -P "$state" -mindepth 1 -print -quit)" ] || { echo 'State directory is not empty' >&2; exit 1; }
  [ ! -e "$activation_marker" ] && [ ! -L "$activation_marker" ] || { echo 'Configuration transition refused after the persistent activation boundary' >&2; exit 1; }
else
  # An operational limit does not bind the store: the manager may have run. What exists is disclosed, not
  # required, and an existing boundary marker must still name this host and this exact candidate.
  [ -z "$(find -P "$state" -mindepth 1 -print -quit)" ] || durable_state_present=true
  if [ -e "$activation_marker" ] || [ -L "$activation_marker" ]; then
    regular_protected "$activation_marker" 600 0 0 || { echo 'Persistent activation marker is unsafe' >&2; exit 2; }
    jq -e --arg alias "$alias_name" --arg version "$package_version" --arg binary "$binary_hash" '.schema_version=="podmesh-manager-activation-boundary/v1" and .host_alias==$alias and .package_version==$version and .binary_sha256==$binary and .rollback_window=="closed"' "$activation_marker" >/dev/null || { echo 'Persistent activation marker does not bind this host and candidate' >&2; exit 2; }
    activation_marker_present=true
  fi
fi
state_listing=$(find -P "$state" -mindepth 1 -printf '%P\0' | sort -z | sha256sum | awk '{print $1}')

unit_state=$(systemctl show podmesh-manager.service --no-page -p LoadState -p ActiveState -p SubState -p UnitFileState -p MainPID -p InvocationID -p NRestarts -p ExecMainStartTimestampMonotonic -p ExecMainExitTimestampMonotonic)
value() { awk -F= -v key="$1" '$1==key {print substr($0,index($0,"=")+1)}' <<<"$unit_state"; }
[ "$(value LoadState)" = loaded ] && [ "$(value ActiveState)" = inactive ] && [ "$(value SubState)" = dead ] && [ "$(value UnitFileState)" = disabled ] && [ "$(value MainPID)" = 0 ] || { echo 'Manager must be loaded, disabled, inactive and process-free' >&2; exit 1; }
for process in /proc/[0-9]*; do
  [ -e "$process/exe" ] || continue
  [ "$(readlink -f -- "$process/exe" 2>/dev/null || true)" != "$binary" ] || { echo 'A manager process exists outside the unit state' >&2; exit 1; }
done
bind=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["network"]["bind"])' "$config")
port=${bind##*:}; [[ "$port" =~ ^[1-9][0-9]{0,4}$ ]] && [ "$port" -le 65535 ] || { echo 'Configured bind port is invalid' >&2; exit 2; }
[ -z "$(ss -H -ltn "sport = :$port")" ] && [ -z "$(ss -H -lun "sport = :$port")" ] || { echo 'Configured manager port already has a listener' >&2; exit 1; }

if [ ! -e "$evidence" ] && [ ! -L "$evidence" ]; then install -d -m 0700 -o root -g root -- "$evidence"; fi
[ -d "$evidence" ] && [ ! -L "$evidence" ] && [ "$(stat -c '%a:%u:%g' -- "$evidence")" = 700:0:0 ] || { echo 'Evidence directory metadata is unsafe' >&2; exit 2; }

hash_file() { sha256sum -- "$1" | awk '{print $1}'; }
commit_file() { { cat -- "$salt"; printf '\000%s\000' "$1"; cat -- "$2"; } | sha256sum | awk '{print "sha256:" $1}'; }
commit_text() { { cat -- "$salt"; printf '\000%s\000%s' "$1" "$2"; } | sha256sum | awk '{print "sha256:" $1}'; }
markers=$(printf '%s' "$unit_state" | sha256sum | awk '{print $1}')
mapping_hash=$(hash_file "$mapping")
report_hash=$(hash_file "$report")
sync_dir() { python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)' "$1"; }
assert_still_quiescent() {
  local current process
  current=$(systemctl show podmesh-manager.service --no-page -p LoadState -p ActiveState -p SubState -p UnitFileState -p MainPID -p InvocationID -p NRestarts -p ExecMainStartTimestampMonotonic -p ExecMainExitTimestampMonotonic)
  [ "$(printf '%s' "$current" | sha256sum | awk '{print $1}')" = "$markers" ] || { echo 'Manager activation markers changed during transition' >&2; exit 1; }
  [ ! -e "$socket" ] && [ ! -L "$socket" ] || { echo 'Manager state or control socket appeared during transition' >&2; exit 1; }
  [ "$(find -P "$state" -mindepth 1 -printf '%P\0' | sort -z | sha256sum | awk '{print $1}')" = "$state_listing" ] || { echo 'Manager state or control socket appeared during transition' >&2; exit 1; }
  if [ "$transition" = three-grants ]; then
    [ ! -e "$activation_marker" ] && [ ! -L "$activation_marker" ] || { echo 'Persistent activation marker appeared during transition' >&2; exit 1; }
  else
    [ "$( { [ -e "$activation_marker" ] || [ -L "$activation_marker" ]; } && echo true || echo false)" = "$activation_marker_present" ] || { echo 'Persistent activation marker changed during transition' >&2; exit 1; }
  fi
  for process in /proc/[0-9]*; do
    [ -e "$process/exe" ] || continue
    [ "$(readlink -f -- "$process/exe" 2>/dev/null || true)" != "$binary" ] || { echo 'A manager process appeared during transition' >&2; exit 1; }
  done
  [ -z "$(ss -H -ltn "sport = :$port")" ] && [ -z "$(ss -H -lun "sport = :$port")" ] || { echo 'Configured manager port gained a listener during transition' >&2; exit 1; }
}

write_ledger() {
  local state_name=$1 original_hash=$2 applied_hash=$3 backup_hash=$4 tmp
  tmp=$(mktemp --tmpdir="$evidence" .ledger.XXXXXX)
  rm -f -- "$tmp"
  python3 - "$tmp" "$state_name" "$alias_name" "$original_hash" "$applied_hash" "$backup_hash" "$mapping_hash" "$report_hash" "$package_version" "$binary_hash" "$markers" "$transition" <<'PY'
import json, os, sys
path, state, alias, original, applied, backup, mapping, report, version, binary, markers, transition = sys.argv[1:]
with open(path, "x", encoding="utf-8") as stream:
    json.dump({"schema_version":"podmesh-manager-config-transition-ledger/v1","transition_kind":transition,"state":state,"host_alias":alias,"original_sha256":original,"applied_sha256":applied,"backup_sha256":backup,"mapping_sha256":mapping,"candidate_report_sha256":report,"package_version":version,"binary_sha256":binary,"activation_markers_sha256":markers,"rollback_window_open":state in {"prepared","applied"}}, stream, sort_keys=True, indent=2)
    stream.write("\n"); stream.flush(); os.fsync(stream.fileno())
PY
  mv -fT -- "$tmp" "$ledger"
  sync_dir "$evidence"
}
read_ledger() {
  regular_protected "$ledger" 600 0 0 || { echo 'A protected transition ledger is required' >&2; exit 2; }
  python3 - "$ledger" "$alias_name" "$mapping_hash" "$report_hash" "$package_version" "$binary_hash" "$transition" <<'PY'
import json, sys
d=json.load(open(sys.argv[1]))
if d.get("schema_version") != "podmesh-manager-config-transition-ledger/v1": raise SystemExit("invalid ledger schema")
if d.get("transition_kind", "three-grants") != sys.argv[7]: raise SystemExit("ledger transition kind mismatch")
if d.get("host_alias") != sys.argv[2] or d.get("mapping_sha256") != sys.argv[3]: raise SystemExit("ledger binding mismatch")
if d.get("candidate_report_sha256") != sys.argv[4] or d.get("package_version") != sys.argv[5] or d.get("binary_sha256") != sys.argv[6]: raise SystemExit("ledger candidate binding mismatch")
if d.get("state") not in {"prepared","applied","sealed-for-activation","rolled-back"}: raise SystemExit("invalid ledger state")
if d.get("rollback_window_open") is not (d.get("state") in {"prepared","applied"}): raise SystemExit("invalid rollback-window state")
for key in ("original_sha256","applied_sha256","backup_sha256","candidate_report_sha256","binary_sha256","activation_markers_sha256"):
    if not isinstance(d.get(key),str) or len(d[key]) != 64 or any(c not in "0123456789abcdef" for c in d[key]): raise SystemExit("invalid ledger digest")
PY
}
atomic_backup_from() {
  local source=$1 temporary
  temporary=$(mktemp --tmpdir=/root .podmesh-manager-config-backup.XXXXXX)
  install -m 0600 -o root -g root -- "$source" "$temporary"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$temporary"
  mv -nT -- "$temporary" "$backup"
  [ ! -e "$temporary" ] || { rm -f -- "$temporary"; echo 'Refusing to overwrite an unbound backup' >&2; exit 1; }
  sync_dir /root
  regular_protected "$backup" 600 0 0 || { echo 'Atomic protected backup creation failed' >&2; exit 1; }
}
validate_offline() {
  local candidate=$1 output created_runtime=0
  if [ ! -e "$runtime" ] && [ ! -L "$runtime" ]; then
    install -d -m 0700 -o podmesh-manager -g podmesh-manager -- "$runtime"
    created_runtime=1
  fi
  [ -d "$runtime" ] && [ ! -L "$runtime" ] && [ "$(stat -c '%a:%u:%g' -- "$runtime")" = "700:$manager_uid:$manager_gid" ] && [ -z "$(find -P "$runtime" -mindepth 1 -print -quit)" ] || { echo 'Canonical validation runtime is unsafe or nonempty' >&2; exit 1; }
  set +e
  output=$(runuser -u podmesh-manager -- "$binary" --config "$candidate" --state-dir "$state" --runtime-dir "$runtime" --validate-config 2>&1)
  rc=$?
  set -e
  if [ "$rc" -ne 0 ] || [ "$output" != '{"configuration_valid":true,"durable_store_checked":false,"network_started":false}' ] || [ -n "$(find -P "$runtime" -mindepth 1 -print -quit)" ]; then
    [ "$created_runtime" -eq 0 ] || rmdir "$runtime" 2>/dev/null || true
    echo 'Offline validation failed or created runtime content' >&2
    exit 1
  fi
  [ "$created_runtime" -eq 0 ] || rmdir "$runtime"
}
atomic_from() {
  local source=$1
  atomic_temporary=$(mktemp --tmpdir=/etc/podmesh-manager .config-transition.XXXXXX)
  install -m 0640 -o root -g podmesh-manager -- "$source" "$atomic_temporary"
  validate_offline "$atomic_temporary"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$atomic_temporary"
  mv -fT -- "$atomic_temporary" "$config"
  atomic_temporary=
  sync_dir /etc/podmesh-manager
  regular_protected "$config" 640 0 "$manager_gid" || { echo 'Final configuration metadata is unsafe' >&2; exit 1; }
  validate_offline "$config"
}
write_result() {
  local action=$1 original=$2 applied=$3 backup_sum=$4 shape=$5 tmp grants sum_tmp grant_alias owner
  tmp=$(mktemp --tmpdir="$evidence" .result.XXXXXX)
  rm -f -- "$tmp"
  grants='[]'
  for grant_alias in lab-a lab-b lab-c; do
    owner=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["aliases"][sys.argv[2]])' "$mapping" "$grant_alias")
    grants=$(jq -cn --argjson current "$grants" --arg scope "g2/$grant_alias/observations" --arg owner "$(commit_text "grant-owner:$grant_alias" "$owner")" '$current+[{scope:$scope,owner_replica_commitment:$owner}]')
  done
  python3 - "$tmp" "$action" "$alias_name" "$(commit_text original-config "$original")" "$(commit_text applied-config "$applied")" "$(commit_text protected-backup "$backup_sum")" "$(commit_file alias-map "$mapping")" "$(commit_file candidate-report "$report")" "$package_version" "$binary_hash" "$(commit_text activation-markers "$markers")" "$shape" "$grants" "$transition" "$durable_state_present" "$activation_marker_present" "$(commit_text state-listing "$state_listing")" <<'PY'
import json, os, sys
path, action, alias, original, applied, backup, mapping, report, version, binary, markers, shape_path, grants_json, transition, durable, marker, listing = sys.argv[1:]
shape=json.load(open(shape_path))
grants=json.loads(grants_json)
document={"schema_version":"podmesh-manager-config-transition-evidence/v1","result":"PASS","action":action,"host_alias":alias,"candidate":{"package":"podmesh-manager","version":version,"binary_sha256":binary,"report_commitment":report,"dpkg_verify":"clean"},"transition":{"source_config_commitment":original,"applied_config_commitment":applied,"protected_backup_commitment":backup,"mapping_commitment":mapping,"activation_markers_commitment":markers,"grant_count":shape["grant_count"],"grants":grants,"all_other_values_preserved":shape["all_other_values_preserved"],"local_replica_matches_mapping":shape["local_replica_matches_mapping"]},"preconditions":{"manager_disabled":True,"manager_inactive":True,"no_manager_process":True,"no_control_socket":True,"no_configured_port_listener":True,"state_directory_empty":True},"offline_validation":{"candidate_valid":True,"final_path_valid":True,"durable_store_checked":False,"network_started":False},"rollback_permitted_only_while":{"state_directory_empty":True,"activation_markers_unchanged":True},"private_values":"absent","claims_not_made":["activation","availability","convergence","DNS","fencing","high availability","replication","takeover"]}
if transition == "incoming-workers":
    # The operational transition discloses what the first-store-open transition had to forbid.
    document["schema_version"]="podmesh-manager-config-transition-evidence/v2"
    document["transition_kind"]=transition
    document["transition"]["changed_keys"]=shape["changed_keys"]
    document["transition"]["incoming_workers"]=shape["incoming_workers"]
    del document["preconditions"]["state_directory_empty"]
    document["preconditions"]["durable_state_present"]=durable=="true"
    document["preconditions"]["activation_marker_present"]=marker=="true"
    # The listing is entry names only, asserted unchanged across the replace: it proves nothing about content.
    document["preconditions"]["state_directory_listing_unchanged"]=True
    document["transition"]["state_listing_commitment"]=listing
    document["rollback_permitted_only_while"]={"activation_markers_unchanged":True}
with open(path,"x",encoding="utf-8") as stream:
    json.dump(document,stream,sort_keys=True,indent=2); stream.write("\n"); stream.flush(); os.fsync(stream.fileno())
PY
  mv -fT -- "$tmp" "$result"
  sync_dir "$evidence"
  sum_tmp=$(mktemp --tmpdir="$evidence" .result-sum.XXXXXX)
  (cd -- "$evidence" && sha256sum -- "${result##*/}") > "$sum_tmp"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$sum_tmp"
  mv -fT -- "$sum_tmp" "$result.sha256"
  sync_dir "$evidence"
}

atomic_temporary=
candidate=$(mktemp --tmpdir=/etc/podmesh-manager .config-shape.XXXXXX)
rm -f -- "$candidate"
shape=$(mktemp --tmpdir="$evidence" .shape.XXXXXX)
cleanup() {
  rm -f -- "$candidate" "$shape"
  [ -z "$atomic_temporary" ] || rm -f -- "$atomic_temporary"
}
trap cleanup EXIT HUP INT TERM

if [ "$mode" = apply ]; then
  if [ ! -e "$ledger" ] && [ ! -L "$ledger" ]; then
    [ ! -e "$backup" ] && [ ! -L "$backup" ] || { echo 'Backup already exists without a transition ledger' >&2; exit 2; }
    python3 "$root/config-tool.py" --transition "$transition" --mode prepare --original "$config" --mapping "$mapping" --host-alias "$alias_name" --output "$candidate"
    chown root:podmesh-manager "$candidate"; chmod 0640 "$candidate"
    validate_offline "$candidate"
    original_hash=$(hash_file "$config"); applied_hash=$(hash_file "$candidate"); backup_hash=$original_hash
    write_ledger prepared "$original_hash" "$applied_hash" "$backup_hash"
    atomic_backup_from "$config"
  else
    read_ledger
    ledger_state=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["state"])' "$ledger")
    case "$ledger_state" in prepared|applied) ;; *) echo 'A closed transition ledger cannot be applied again' >&2; exit 2;; esac
    original_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["original_sha256"])' "$ledger")
    applied_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["applied_sha256"])' "$ledger")
    backup_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["backup_sha256"])' "$ledger")
    if [ ! -e "$backup" ] && [ ! -L "$backup" ] && [ "$(hash_file "$config")" = "$original_hash" ]; then
      atomic_backup_from "$config"
    fi
    regular_protected "$backup" 600 0 0 || { echo 'Protected backup is missing or unsafe' >&2; exit 2; }
    [ "$(hash_file "$backup")" = "$backup_hash" ] && [ "$markers" = "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["activation_markers_sha256"])' "$ledger")" ] || { echo 'Backup or activation markers changed' >&2; exit 1; }
    python3 "$root/config-tool.py" --transition "$transition" --mode prepare --original "$backup" --mapping "$mapping" --host-alias "$alias_name" --output "$candidate"
    [ "$(hash_file "$candidate")" = "$applied_hash" ] || { echo 'Resumed candidate differs from the ledger' >&2; exit 1; }
    chown root:podmesh-manager "$candidate"; chmod 0640 "$candidate"
    validate_offline "$candidate"
  fi
  current_hash=$(hash_file "$config")
  assert_still_quiescent
  if [ "$current_hash" = "$original_hash" ]; then atomic_from "$candidate"; elif [ "$current_hash" = "$applied_hash" ]; then validate_offline "$config"; else echo 'Current configuration is neither the original nor the exact candidate' >&2; exit 1; fi
  python3 "$root/config-tool.py" --transition "$transition" --mode verify --original "$backup" --candidate "$config" --mapping "$mapping" --host-alias "$alias_name" > "$shape"
  assert_still_quiescent
  write_ledger applied "$original_hash" "$applied_hash" "$backup_hash"
  write_result applied "$original_hash" "$applied_hash" "$backup_hash" "$shape"
  echo CONFIG_TRANSITION_APPLIED
  exit 0
fi

read_ledger
ledger_state=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["state"])' "$ledger")
if [ "$mode" = rollback ] && [ "$ledger_state" = sealed-for-activation ]; then
  echo 'Rollback refused: the durable window was closed before activation' >&2
  exit 1
fi
regular_protected "$backup" 600 0 0 || { echo 'Protected backup is missing or unsafe' >&2; exit 2; }
original_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["original_sha256"])' "$ledger")
applied_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["applied_sha256"])' "$ledger")
backup_hash=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["backup_sha256"])' "$ledger")
[ "$(hash_file "$backup")" = "$backup_hash" ] && [ "$markers" = "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["activation_markers_sha256"])' "$ledger")" ] || { echo 'Rollback refused: backup or activation markers changed' >&2; exit 1; }
python3 "$root/config-tool.py" --transition "$transition" --mode prepare --original "$backup" --mapping "$mapping" --host-alias "$alias_name" --output "$candidate"
[ "$(hash_file "$candidate")" = "$applied_hash" ] || { echo 'Rollback candidate binding failed' >&2; exit 1; }
python3 "$root/config-tool.py" --transition "$transition" --mode verify --original "$backup" --candidate "$candidate" --mapping "$mapping" --host-alias "$alias_name" > "$shape"
current_hash=$(hash_file "$config")
if [ "$mode" = seal-for-activation ]; then
  [ "$ledger_state" = applied ] && [ "$current_hash" = "$applied_hash" ] || { echo 'Activation sealing requires the exact applied configuration and open rollback window' >&2; exit 1; }
  validate_offline "$config"
  assert_still_quiescent
  write_ledger sealed-for-activation "$original_hash" "$applied_hash" "$backup_hash"
  seal_result=$evidence/rollback-window-closed.json
  seal_tmp=$(mktemp --tmpdir="$evidence" .seal.XXXXXX)
  rm -f -- "$seal_tmp"
  python3 - "$seal_tmp" "$alias_name" "$(commit_file candidate-report "$report")" "$(commit_text activation-markers "$markers")" "$transition" "$durable_state_present" "$activation_marker_present" <<'PY'
import json, os, sys
path, alias, candidate, markers, transition, durable, marker = sys.argv[1:]
document={"schema_version":"podmesh-manager-rollback-window-closure/v1","result":"PASS","host_alias":alias,"rollback_window":"permanently-closed-for-this-transition","candidate_report_commitment":candidate,"activation_markers_commitment":markers,"state_directory_empty_at_closure":True,"manager_inactive_at_closure":True,"private_values":"absent"}
if transition != "three-grants":
    # Only the first-store-open transition can assert an empty state directory, because only it requires one.
    # Any other kind names itself and discloses what it actually found; v1 is emitted by three-grants alone.
    document["schema_version"]="podmesh-manager-rollback-window-closure/v2"
    document["transition_kind"]=transition
    del document["state_directory_empty_at_closure"]
    document["durable_state_present_at_closure"]=durable=="true"
    document["activation_marker_present_at_closure"]=marker=="true"
with open(path,"x",encoding="utf-8") as stream:
    json.dump(document,stream,sort_keys=True,indent=2)
    stream.write("\n"); stream.flush(); os.fsync(stream.fileno())
PY
  mv -fT -- "$seal_tmp" "$seal_result"
  sync_dir "$evidence"
  echo CONFIG_TRANSITION_SEALED_FOR_ACTIVATION
  exit 0
fi
if [ "$current_hash" = "$applied_hash" ]; then atomic_from "$backup"; elif [ "$current_hash" = "$original_hash" ]; then validate_offline "$config"; else echo 'Rollback refused: current configuration has drifted' >&2; exit 1; fi
assert_still_quiescent
write_ledger rolled-back "$original_hash" "$applied_hash" "$backup_hash"
write_result rolled-back "$original_hash" "$applied_hash" "$backup_hash" "$shape"
echo CONFIG_TRANSITION_ROLLED_BACK
