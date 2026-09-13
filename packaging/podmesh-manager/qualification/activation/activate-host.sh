#!/bin/bash
# Controlled manager2 activation and resumable, application-level rollback.
set -euo pipefail
export LC_ALL=C
umask 077

root=$(cd -- "$(dirname -- "$0")" && pwd)
usage() { echo "Usage: $0 [--mode activate|seal-converged|rollback] --host-alias <stable-alias> --salt-file <private-salt> --candidate-verification <report.json> --evidence-directory <directory> [--dropin-source <file>]" >&2; exit 2; }
mode=activate alias_name= salt= report= evidence= source=
while [ "$#" -gt 0 ]; do case "$1" in
  --mode) mode=${2-}; shift 2;; --host-alias) alias_name=${2-}; shift 2;; --salt-file) salt=${2-}; shift 2;;
  --candidate-verification) report=${2-}; shift 2;; --evidence-directory) evidence=${2-}; shift 2;; --dropin-source) source=${2-}; shift 2;; *) usage;; esac; done
case "$mode" in activate|seal-converged|rollback) ;; *) usage;; esac
[ "$(id -u)" -eq 0 ] && [[ "$alias_name" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$ ]] && [ -f "$salt" ] && [ -f "$report" ] && [ -n "$evidence" ] || usage
[ "$mode" != activate ] || [ -f "$source" ] || usage
for command in systemctl sha256sum jq install rm mkdir mv rmdir find grep python3 flock stat mktemp chmod chown readlink; do command -v "$command" >/dev/null || { echo "$command is required" >&2; exit 2; }; done

lock_dir=/run/podmesh-manager-qualification
if [ ! -e "$lock_dir" ] && [ ! -L "$lock_dir" ]; then install -d -m 0700 -o root -g root -- "$lock_dir"; fi
[ -d "$lock_dir" ] && [ ! -L "$lock_dir" ] && [ "$(stat -c '%a:%u:%g' -- "$lock_dir")" = 700:0:0 ] || { echo 'Manager qualification lock directory is unsafe' >&2; exit 2; }
exec 9>"$lock_dir/manager2.lock"
[ "$(stat -c '%F:%a:%u:%g:%h' -- "$lock_dir/manager2.lock")" = 'regular empty file:600:0:0:1' ] || { echo 'Shared manager2 lock file is unsafe' >&2; exit 2; }
flock -n 9 || { echo 'Another manager transition or activation is running' >&2; exit 1; }

dropin_dir=/etc/systemd/system/podmesh-manager.service.d
dropin=$dropin_dir/90-g2-network.conf
dropin_tmp=$dropin_dir/.90-g2-network.conf.new
ledger=$evidence/activation-ledger.json
ledger_sum=$ledger.sha256
activation_marker=/etc/podmesh-manager/.manager2-activation-started
version=$(jq -er 'select(.schema_version=="podmesh-manager-candidate-verification/v2" and .package=="podmesh-manager")|.version' "$report") || { echo 'Candidate report is invalid' >&2; exit 2; }
binary_hash=$(jq -er '.binary_sha256|select(test("^[a-f0-9]{64}$"))' "$report") || { echo 'Candidate report binary hash is invalid' >&2; exit 2; }

capture() {
  local stage=$1 filename=$2 inspection=${3-}
  local args=(--host-alias "$alias_name" --stage "$stage" --salt-file "$salt" --candidate-verification "$report" --output "$evidence/$filename")
  [ "$inspection" != with-inspection ] || args+=(--with-inspection)
  "$root/capture-host.sh" "${args[@]}"
}
write_ledger() {
  local state=$1 extra=${2-'{}'} temporary=$ledger.new current='{}'
  [ ! -f "$ledger" ] || current=$(cat -- "$ledger")
  jq -nS --arg alias "$alias_name" --arg state "$state" --arg version "$version" --arg binary "$binary_hash" --argjson current "$current" --argjson extra "$extra" '$current+{schema_version:"podmesh-manager-live-activation-ledger/v2",host_alias:$alias,state:$state,package_version:$version,binary_sha256:$binary}+ $extra' > "$temporary"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$temporary"
  mv -- "$temporary" "$ledger"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)' "$evidence"
  sha256sum -- "$ledger" > "$ledger_sum.new"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$ledger_sum.new"
  mv -- "$ledger_sum.new" "$ledger_sum"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)' "$evidence"
}
mark_activation_started() {
  local temporary marker_dir
  marker_dir=${activation_marker%/*}
  if [ -e "$activation_marker" ] || [ -L "$activation_marker" ]; then
    [ -f "$activation_marker" ] && [ ! -L "$activation_marker" ] && [ "$(stat -c '%a:%u:%g:%h' -- "$activation_marker")" = 600:0:0:1 ] || { echo 'Persistent activation marker is unsafe' >&2; exit 2; }
    jq -e --arg alias "$alias_name" --arg version "$version" --arg binary "$binary_hash" '.schema_version=="podmesh-manager-activation-boundary/v1" and .host_alias==$alias and .package_version==$version and .binary_sha256==$binary and .rollback_window=="closed"' "$activation_marker" >/dev/null || { echo 'Persistent activation marker does not bind this host and candidate' >&2; exit 2; }
    return
  fi
  temporary=$(mktemp --tmpdir="$marker_dir" .manager2-activation-started.XXXXXX)
  rm -f -- "$temporary"
  jq -nS --arg alias "$alias_name" --arg version "$version" --arg binary "$binary_hash" '{schema_version:"podmesh-manager-activation-boundary/v1",host_alias:$alias,package_version:$version,binary_sha256:$binary,rollback_window:"closed"}' > "$temporary"
  chmod 0600 "$temporary"; chown root:root "$temporary"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$temporary"
  mv -nT -- "$temporary" "$activation_marker"
  [ ! -e "$temporary" ] || { rm -f -- "$temporary"; echo 'Persistent activation marker appeared concurrently' >&2; exit 1; }
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)' "$marker_dir"
}
read_ledger() {
  [ -f "$ledger" ] && [ ! -L "$ledger" ] && [ "$(stat -c '%a:%u:%g:%h' -- "$ledger")" = 600:0:0:1 ] || { echo 'Rollback requires a protected activation ledger' >&2; exit 2; }
  if [ ! -f "$ledger_sum" ] || [ -L "$ledger_sum" ] || ! (cd -- "$evidence" && sha256sum -c -- "${ledger_sum##*/}" >/dev/null 2>&1); then
    jq -e --arg alias "$alias_name" --arg version "$version" --arg binary "$binary_hash" '.schema_version=="podmesh-manager-live-activation-ledger/v2" and .host_alias==$alias and .package_version==$version and .binary_sha256==$binary and (.state|IN("prepared","dropin-installed","start-failed","active","converged","stopped","dropin-removed","complete","failed-start-cleaned"))' "$ledger" >/dev/null || { echo 'Activation ledger is invalid and its checksum cannot be repaired' >&2; exit 2; }
    sha256sum -- "$ledger" > "$ledger_sum.recovered"
    chmod 0600 "$ledger_sum.recovered"; chown root:root "$ledger_sum.recovered"
    mv -fT -- "$ledger_sum.recovered" "$ledger_sum"
    python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY); os.fsync(fd); os.close(fd)' "$evidence"
  fi
  jq -e --arg alias "$alias_name" --arg version "$version" --arg binary "$binary_hash" '.schema_version=="podmesh-manager-live-activation-ledger/v2" and .host_alias==$alias and .package_version==$version and .binary_sha256==$binary' "$ledger" >/dev/null || { echo 'Activation ledger does not bind this host and candidate' >&2; exit 2; }
}

if [ "$mode" = activate ]; then
  [ ! -e "$ledger" ] && [ ! -e "$ledger_sum" ] || { echo 'Activation ledger already exists; use a fresh evidence directory or finish rollback' >&2; exit 2; }
  [ ! -e "$dropin" ] && [ ! -L "$dropin" ] || { echo 'Refusing to replace an existing manager activation drop-in' >&2; exit 2; }
  if [ -e "$dropin_dir" ] && [ ! -d "$dropin_dir" ]; then echo 'Manager drop-in path is unsafe' >&2; exit 2; fi
  if [ -d "$dropin_dir" ] && find "$dropin_dir" -mindepth 1 -print -quit | grep -q .; then echo 'Refusing to activate with another manager drop-in present' >&2; exit 2; fi
  "$root/validate-dropin.py" --dropin "$source" --quiet
  if [ ! -e "$evidence" ] && [ ! -L "$evidence" ]; then install -d -m 0700 -o root -g root -- "$evidence"; fi
  [ -d "$evidence" ] && [ ! -L "$evidence" ] && [ "$(stat -c '%a:%u:%g' -- "$evidence")" = 700:0:0 ] || { echo 'Activation evidence directory is unsafe' >&2; exit 2; }
  capture pre-activation pre-activation.json
  jq -e '.service.active_state=="inactive" and .service.sub_state=="dead" and .service.unit_file_state=="disabled" and .manager_process.count==0 and (.dropin.present|not)' "$evidence/pre-activation.json" >/dev/null || { echo 'Manager is not in the required disabled/inactive precondition' >&2; exit 2; }
  expected=$(sha256sum -- "$source" | awk '{print $1}')
  write_ledger prepared "$(jq -n --arg dropin "$expected" --arg pre "$(awk '{print $1}' "$evidence/pre-activation.json.sha256")" '{dropin_sha256:$dropin,pre_evidence_sha256:$pre}')"
  install -d -m 0755 -- "$dropin_dir"
  install -m 0644 -- "$source" "$dropin_tmp"
  mv -- "$dropin_tmp" "$dropin"
  [ "$(sha256sum -- "$dropin" | awk '{print $1}')" = "$expected" ] || { echo 'Installed drop-in hash mismatch' >&2; exit 1; }
  systemctl daemon-reload
  write_ledger dropin-installed "$(jq -n --arg dropin "$expected" '{dropin_sha256:$dropin}')"
  mark_activation_started
  if ! systemctl start podmesh-manager.service; then
    start_failure=$(systemctl show podmesh-manager.service --no-page -p ActiveState -p SubState -p MainPID -p Result -p ExecMainCode -p ExecMainStatus | sha256sum | awk '{print $1}')
    write_ledger start-failed "$(jq -n --arg dropin "$expected" --arg failure "$start_failure" '{dropin_sha256:$dropin,start_failure_state_sha256:$failure}')"
    echo 'Start failed and was recorded; run the resumable rollback mode' >&2
    exit 1
  fi
  "$root/wait-ready.py" > "$evidence/readiness.json" || { echo 'Readiness failed; run the resumable rollback mode' >&2; exit 1; }
  capture active-baseline active-baseline.json with-inspection || { echo 'Active capture failed; run the resumable rollback mode' >&2; exit 1; }
  write_ledger active "$(jq -n --arg dropin "$expected" --arg pre "$(awk '{print $1}' "$evidence/pre-activation.json.sha256")" --arg readiness "$(sha256sum "$evidence/readiness.json"|awk '{print $1}')" --arg active "$(awk '{print $1}' "$evidence/active-baseline.json.sha256")" '{dropin_sha256:$dropin,pre_evidence_sha256:$pre,readiness_sha256:$readiness,active_baseline_evidence_sha256:$active}')"
  exit 0
fi

read_ledger
state=$(jq -r .state "$ledger")
case "$state" in prepared|dropin-installed|start-failed|active|converged|stopped|dropin-removed|complete|failed-start-cleaned) ;; *) echo 'Activation ledger state is invalid' >&2; exit 2;; esac
if [ "$mode" = seal-converged ]; then
  [ "$state" = active ] || { echo 'Converged capture can only seal an active ledger' >&2; exit 2; }
  converged=$evidence/converged.json
  [ -f "$converged" ] && [ -f "$converged.sha256" ] || { echo 'Converged evidence and sidecar are required' >&2; exit 2; }
  (cd -- "$evidence" && sha256sum -c -- converged.json.sha256 >/dev/null) || { echo 'Converged evidence checksum mismatch' >&2; exit 2; }
  jq -e --arg alias "$alias_name" --slurpfile baseline "$evidence/active-baseline.json" '.schema_version=="podmesh-manager-live-activation-evidence/v2" and .stage=="converged" and .host_alias==$alias and .package==$baseline[0].package and .configuration==$baseline[0].configuration' "$converged" >/dev/null || { echo 'Converged evidence does not bind the active host and candidate' >&2; exit 2; }
  write_ledger converged "$(jq -n --arg converged "$(awk '{print $1}' "$converged.sha256")" '{converged_evidence_sha256:$converged}')"
  exit 0
fi
[ "$state" != complete ] && [ "$state" != failed-start-cleaned ] || exit 0
expected=$(jq -r '.dropin_sha256' "$ledger")
[[ "$expected" =~ ^[a-f0-9]{64}$ ]] || { echo 'Activation ledger drop-in hash is invalid' >&2; exit 2; }

if [ "$state" = start-failed ]; then
  main_pid=$(systemctl show podmesh-manager.service -p MainPID --value)
  if [ "$main_pid" != 0 ]; then
    shutdown_report=$evidence/failed-start-graceful-shutdown.json
    "$root/graceful-shutdown.py" > "$shutdown_report" || { echo 'Failed start left a live manager that did not shut down cleanly' >&2; exit 1; }
  else
    active_state=$(systemctl show podmesh-manager.service -p ActiveState --value)
    case "$active_state" in inactive|failed) ;; *) echo 'Failed start remains in a transitional service state' >&2; exit 1;; esac
    for process in /proc/[0-9]*; do
      [ -e "$process/exe" ] || continue
      [ "$(readlink -f -- "$process/exe" 2>/dev/null || true)" != /usr/lib/podmesh-manager/podmesh-managerd ] || { echo 'Failed start left an unmanaged manager process' >&2; exit 1; }
    done
  fi
  if [ -e "$dropin" ] || [ -L "$dropin" ]; then
    [ -f "$dropin" ] && [ ! -L "$dropin" ] && [ "$(sha256sum -- "$dropin"|awk '{print $1}')" = "$expected" ] || { echo 'Failed start cleanup found an unowned drop-in' >&2; exit 1; }
    rm -f -- "$dropin"
  fi
  rmdir "$dropin_dir" 2>/dev/null || true
  systemctl daemon-reload
  systemctl reset-failed podmesh-manager.service
  [ "$(systemctl show podmesh-manager.service -p ActiveState --value)" = inactive ] && [ "$(systemctl show podmesh-manager.service -p SubState --value)" = dead ] && [ "$(systemctl show podmesh-manager.service -p MainPID --value)" = 0 ] || { echo 'Failed start cleanup did not restore an inactive service state' >&2; exit 1; }
  jq -nS --arg alias "$alias_name" --arg version "$version" --arg binary "$binary_hash" '{schema_version:"podmesh-manager-failed-start-cleanup/v1",status:"RECOVERED_NOT_QUALIFIED",host_alias:$alias,package_version:$version,binary_sha256:$binary,dropin_removed:true,activation_claim:false}' > "$evidence/failed-start-cleanup.json"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$evidence/failed-start-cleanup.json"
  sha256sum -- "$evidence/failed-start-cleanup.json" > "$evidence/failed-start-cleanup.json.sha256"
  python3 -c 'import os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); os.fsync(fd); os.close(fd)' "$evidence/failed-start-cleanup.json.sha256"
  write_ledger failed-start-cleaned "$(jq -n --arg dropin "$expected" '{dropin_sha256:$dropin,activation_claim:false}')"
  exit 0
fi

if [[ "$state" == dropin-installed || "$state" == active || "$state" == converged ]]; then
  if [ "$(systemctl show podmesh-manager.service -p MainPID --value)" != 0 ]; then
    shutdown_report=$evidence/graceful-shutdown.json
    "$root/graceful-shutdown.py" > "$shutdown_report" || { echo 'Graceful shutdown failed; retaining activation state and drop-in' >&2; exit 1; }
    write_ledger stopped "$(jq -n --arg dropin "$expected" --arg shutdown "$(sha256sum "$shutdown_report"|awk '{print $1}')" '{dropin_sha256:$dropin,graceful_shutdown_sha256:$shutdown}')"
  else
    echo 'An activated ledger has no live process and no typed shutdown proof; refusing to certify cleanup' >&2
    exit 1
  fi
  state=stopped
fi
if [ "$state" = prepared ]; then
  if [ -e "$dropin_tmp" ] || [ -L "$dropin_tmp" ]; then
    [ -f "$dropin_tmp" ] && [ ! -L "$dropin_tmp" ] && [ "$(sha256sum -- "$dropin_tmp"|awk '{print $1}')" = "$expected" ] || { echo 'Prepared ledger found an unowned temporary drop-in' >&2; exit 1; }
    rm -f -- "$dropin_tmp"
  fi
  if [ -e "$dropin" ]; then
    [ -f "$dropin" ] && [ ! -L "$dropin" ] && [ "$(sha256sum -- "$dropin"|awk '{print $1}')" = "$expected" ] || { echo 'Prepared ledger found an unowned drop-in' >&2; exit 1; }
    [ "$(systemctl show podmesh-manager.service -p MainPID --value)" = 0 ] || { echo 'Prepared ledger unexpectedly has a live process' >&2; exit 1; }
    rm -f -- "$dropin"
    rmdir "$dropin_dir" 2>/dev/null || true
    systemctl daemon-reload
  fi
  write_ledger dropin-removed "$(jq -n --arg dropin "$expected" '{dropin_sha256:$dropin}')"
  state=dropin-removed
fi
if [ "$state" = stopped ]; then
  if [ -e "$dropin" ] || [ -L "$dropin" ]; then
    [ -f "$dropin" ] && [ ! -L "$dropin" ] || { echo 'Harness activation drop-in is unsafe' >&2; exit 2; }
    [ "$(sha256sum -- "$dropin"|awk '{print $1}')" = "$expected" ] || { echo 'Refusing to remove a drop-in whose hash differs from the activation ledger' >&2; exit 2; }
    rm -f -- "$dropin"
  fi
  shutdown_hash=$(jq -r .graceful_shutdown_sha256 "$ledger")
  rmdir "$dropin_dir" 2>/dev/null || true
  systemctl daemon-reload
  write_ledger dropin-removed "$(jq -n --arg dropin "$expected" --arg shutdown "$shutdown_hash" '{dropin_sha256:$dropin,graceful_shutdown_sha256:$shutdown}')"
  state=dropin-removed
fi
if [ "$state" = dropin-removed ]; then
  shutdown_report=$evidence/graceful-shutdown.json
  [ -f "$shutdown_report" ] || { echo 'Post-activation cleanup lacks typed shutdown evidence' >&2; exit 1; }
  "$root/capture-host.sh" --host-alias "$alias_name" --stage post-cleanup --salt-file "$salt" --candidate-verification "$report" --output "$evidence/post-cleanup.json" --with-inspection --shutdown-report "$shutdown_report"
  write_ledger complete "$(jq -n --arg dropin "$expected" --arg cleanup "$(awk '{print $1}' "$evidence/post-cleanup.json.sha256")" '{dropin_sha256:$dropin,post_cleanup_evidence_sha256:$cleanup}')"
fi
