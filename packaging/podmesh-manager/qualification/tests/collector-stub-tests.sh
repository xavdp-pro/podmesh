#!/bin/bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work/bin" "$work/proc"
printf '%032d' 0 > "$work/salt"
chmod 600 "$work/salt"

cat > "$work/bin/podman" <<'EOF'
#!/bin/sh
[ "${LC_ALL-}" = C ] || { printf '%s\n' 'collector did not force the C locale' >&2; exit 76; }
case "${PODMESH_STUB_MODE:-failure}" in
  failure) exit 73 ;;
  empty) exit 0 ;;
  non-array) printf '%s\n' '{"unexpected":true}'; exit 0 ;;
  success) printf '%s\n' '[]'; exit 0 ;;
esac
EOF
chmod +x "$work/bin/podman"
cat > "$work/bin/id" <<'EOF'
#!/bin/sh
[ "${1-}" = -u ] && { printf '%s\n' 0; exit 0; }
exec /usr/bin/id "$@"
EOF
cat > "$work/bin/stat" <<'EOF'
#!/bin/sh
if [ "${1-}" = -c ] && [ "${2-}" = '%a:%u:%F' ]; then
  printf '%s\n' '600:0:regular file'
  exit 0
fi
exec /usr/bin/stat "$@"
EOF
cat > "$work/bin/getent" <<'EOF'
#!/bin/sh
if [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ]; then
  [ "${1-}" = passwd ] && { printf '%s\n' 'podmesh-manager:x:994:994::/nonexistent:/usr/sbin/nologin'; exit 0; }
  [ "${1-}" = group ] && { printf '%s\n' 'podmesh-manager:x:994:'; exit 0; }
fi
case "${PODMESH_GETENT_MODE:-system}" in
  failure) exit 75 ;;
  malformed)
    [ "${1-}" = passwd ] && { printf '%s\n' 'podmesh-manager:x:not-a-uid:994::/nonexistent:/usr/sbin/nologin'; exit 0; }
    [ "${1-}" = group ] && { printf '%s\n' 'podmesh-manager:x:994:'; exit 0; }
    ;;
esac
exec /usr/bin/getent "$@"
EOF
cat > "$work/bin/readlink" <<'EOF'
#!/bin/sh
case "${2-}" in
  */proc/8/exe|*/proc/9/exe|*/proc/10/exe|*/proc/11/exe) exit 1 ;;
esac
exec /usr/bin/readlink "$@"
EOF
cat > "$work/bin/dpkg-query" <<'EOF'
#!/bin/sh
case "${1-}" in
  -L|--listfiles)
    [ "${2-}" = podmesh-manager ] && [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ] || exit 72
    printf '%s\n' /usr/lib/podmesh-manager/podmesh-managerd /usr/lib/systemd/system/podmesh-manager.service
    exit 0
    ;;
  --control-list)
    [ "${2-}" = podmesh-manager ] && [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ] || exit 72
    printf '%s\n' md5sums postinst
    exit 0
    ;;
  --control-path|-c)
    [ "${2-}" = podmesh-manager ] && [ "${3-}" = postinst ] && [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ] || exit 72
    printf '%s\n' "${PODMESH_TEST_CONTROL_ROOT:?}/postinst"
    exit 0
    ;;
esac
format=${2-}
package=${3-}
case "$package:$format" in
  podmesh:*Status*) printf '%s\n' installed ;;
  podmesh:*Version*) printf '%s\n' 1.0 ;;
  podmesh-web-observer:*Status*) printf '%s\n' installed ;;
  podmesh-web-observer:*Version*) printf '%s\n' 2.0 ;;
  podmesh-manager:*Status*)
    if [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ]; then printf '%s\n' installed; else printf '%s\n' 'dpkg-query: no packages found matching podmesh-manager' >&2; exit 1; fi
    ;;
  podmesh-manager:*Version*) [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ] && { printf '%s\n' '0.1'; exit 0; } || exit 72 ;;
  *) exit 72 ;;
esac
EOF
cat > "$work/bin/dpkg" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$work/bin/systemctl" <<'EOF'
#!/bin/sh
unit=${2-}
if [ -n "${PODMESH_SYSTEMCTL_COUNTER:-}" ]; then
  count=0
  [ ! -f "$PODMESH_SYSTEMCTL_COUNTER" ] || count=$(cat -- "$PODMESH_SYSTEMCTL_COUNTER")
  count=$((count + 1))
  printf '%s\n' "$count" > "$PODMESH_SYSTEMCTL_COUNTER"
else
  count=0
fi
case "$unit" in
  podmesh.service)
    main_pid=101
    if [ "${PODMESH_MUTATE_HOST_EVIDENCE:-0}" = 1 ] && [ "$count" -gt 3 ]; then main_pid=303; fi
    cat <<'OUT'
LoadState=loaded
ActiveState=active
SubState=running
UnitFileState=enabled
OUT
    printf 'MainPID=%s\nExecMainPID=%s\n' "$main_pid" "$main_pid"
    cat <<'OUT'
InvocationID=11111111111111111111111111111111
ExecMainStartTimestampMonotonic=1000000
NRestarts=0
OUT
    ;;
  podmesh-web-observer.service)
    cat <<'OUT'
LoadState=loaded
ActiveState=active
SubState=running
UnitFileState=enabled
MainPID=202
ExecMainPID=202
InvocationID=22222222222222222222222222222222
ExecMainStartTimestampMonotonic=2000000
NRestarts=0
OUT
    ;;
  podmesh-manager.service)
    if [ "${PODMESH_MANAGER_INSTALLED:-0}" = 1 ]; then
      cat <<'OUT'
LoadState=loaded
ActiveState=inactive
SubState=dead
UnitFileState=disabled
MainPID=0
ExecMainPID=0
InvocationID=
ExecMainStartTimestampMonotonic=0
NRestarts=0
OUT
      exit 0
    fi
    cat <<OUT
LoadState=not-found
ActiveState=inactive
SubState=dead
UnitFileState=
${PODMESH_SYSTEMCTL_MAIN_PID_LINE-MainPID=0}
ExecMainPID=0
InvocationID=
ExecMainStartTimestampMonotonic=0
NRestarts=0
OUT
    ;;
  *) exit 74 ;;
esac
EOF
cat > "$work/bin/sha256sum" <<'EOF'
#!/bin/sh
if [ "$#" -gt 0 ]; then exec /usr/bin/sha256sum "$@"; fi
material=$(mktemp)
trap 'rm -f -- "$material"' EXIT
cat > "$material"
/usr/bin/sha256sum -- "$material" | awk '{print $1 "  -"}'
if [ -n "${PODMESH_COMMIT_COUNTER:-}" ] && grep -aFq -- "${PODMESH_COMMIT_LABEL:-unused}" "$material"; then
  count=0
  [ ! -f "$PODMESH_COMMIT_COUNTER" ] || count=$(cat -- "$PODMESH_COMMIT_COUNTER")
  count=$((count + 1))
  printf '%s\n' "$count" > "$PODMESH_COMMIT_COUNTER"
  if [ "$count" -eq 1 ] && [ -n "${PODMESH_MUTATION_TARGET:-}" ]; then
    printf '%s\n' 'concurrent mutation' >> "$PODMESH_MUTATION_TARGET"
  fi
fi
EOF
chmod +x "$work/bin/id" "$work/bin/stat" "$work/bin/getent" "$work/bin/readlink" "$work/bin/dpkg-query" "$work/bin/dpkg" "$work/bin/systemctl" "$work/bin/sha256sum"

printf '%s' weak > "$work/weak-salt"
chmod 600 "$work/weak-salt"
if PATH="$work/bin:$PATH" PODMESH_PROC_ROOT="$work/proc" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/weak-salt" --output "$work/weak-salt.json" >"$work/weak-salt.out" 2>"$work/weak-salt.err"; then
  echo "collector accepted a salt shorter than 32 bytes" >&2
  exit 1
fi
grep -Fq 'Salt file must contain at least 32 bytes' "$work/weak-salt.err"

PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_PROC_ROOT="$work/proc" PODMESH_PATH_ROOT="$work/fs" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/success.json"
jq -e '.schema_version == "podmesh-manager-host-evidence/v5" and .manager.process_count == 0 and (.manager.config_present | not) and (.manager.runtime_present | not)' "$work/success.json" >/dev/null
python3 - "$root/evidence-schema.json" "$work/success.json" <<'PY'
import json
import sys
from pathlib import Path
from jsonschema import Draft202012Validator, FormatChecker

schema = json.loads(Path(sys.argv[1]).read_text())
evidence = json.loads(Path(sys.argv[2]).read_text())
errors = list(Draft202012Validator(schema, format_checker=FormatChecker()).iter_errors(evidence))
if errors:
    raise SystemExit(f"schema rejected positive collector output: {errors[0].message}")
PY

if PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_SYSTEMCTL_MAIN_PID_LINE= PODMESH_PROC_ROOT="$work/proc" PODMESH_PATH_ROOT="$work/fs" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/missing-pid.json" >"$work/missing-pid.out" 2>"$work/missing-pid.err"; then
  echo "collector accepted a missing MainPID property" >&2
  exit 1
fi
test ! -e "$work/missing-pid.json"
grep -Fq 'incomplete systemd unit reading' "$work/missing-pid.err"

mkdir -p "$work/fs/etc/podmesh-manager" "$work/fs/run"
ln -s /missing-config-target "$work/fs/etc/podmesh-manager/config.json"
ln -s /missing-runtime-target "$work/fs/run/podmesh-manager"
PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_PROC_ROOT="$work/proc" PODMESH_PATH_ROOT="$work/fs" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/dangling-paths.json"
jq -e '.manager.config_present == true and .manager.runtime_present == true' "$work/dangling-paths.json" >/dev/null

mkdir -p "$work/proc/7" "$work/proc/8" "$work/proc/9"
ln -s /usr/lib/podmesh-manager/podmesh-managerd "$work/proc/7/exe"
# Kernel threads and zombies both expose an unresolved executable and an empty command line.
ln -s /kernel-thread "$work/proc/8/exe"
ln -s /terminated-process "$work/proc/9/exe"
touch "$work/proc/7/cmdline" "$work/proc/8/cmdline" "$work/proc/9/cmdline"
PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_PROC_ROOT="$work/proc" PODMESH_PATH_ROOT="$work/empty-fs" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/renamed-process.json"
jq -e '.manager.process_count == 1' "$work/renamed-process.json" >/dev/null
rm -- "$work/proc/7/exe" "$work/proc/7/cmdline" "$work/proc/8/exe" "$work/proc/8/cmdline" "$work/proc/9/exe" "$work/proc/9/cmdline"
rmdir -- "$work/proc/7" "$work/proc/8" "$work/proc/9"

mkdir -p "$work/proc/10"
ln -s /running-process "$work/proc/10/exe"
printf '/usr/lib/podmesh-manager/podmesh-managerd\0--worker\0' > "$work/proc/10/cmdline"
if PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_PROC_ROOT="$work/proc" PODMESH_PATH_ROOT="$work/empty-fs" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/unresolved-live-process.json" >"$work/unresolved-live-process.out" 2>"$work/unresolved-live-process.err"; then
  echo "collector accepted a live process whose executable could not be resolved" >&2
  exit 1
fi
test ! -e "$work/unresolved-live-process.json"
grep -Fq 'Cannot read executable for process 10' "$work/unresolved-live-process.err"
rm -- "$work/proc/10/exe" "$work/proc/10/cmdline"
rmdir -- "$work/proc/10"

mkdir -p "$work/proc/11"
ln -s /running-process "$work/proc/11/exe"
ln -s /missing-cmdline-target "$work/proc/11/cmdline"
if PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_PROC_ROOT="$work/proc" PODMESH_PATH_ROOT="$work/empty-fs" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/unreadable-cmdline.json" >"$work/unreadable-cmdline.out" 2>"$work/unreadable-cmdline.err"; then
  echo "collector accepted an unreadable command line after executable lookup failed" >&2
  exit 1
fi
test ! -e "$work/unreadable-cmdline.json"
grep -Fq 'Cannot read command line for process 11 after executable lookup failed' "$work/unreadable-cmdline.err"
rm -- "$work/proc/11/exe" "$work/proc/11/cmdline"
rmdir -- "$work/proc/11"

for mode in failure malformed; do
  if PATH="$work/bin:$PATH" PODMESH_GETENT_MODE="$mode" PODMESH_PROC_ROOT="$work/proc" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/getent-$mode.json" >"$work/getent-$mode.out" 2>"$work/getent-$mode.err"; then
    echo "collector accepted $mode manager account evidence" >&2
    exit 1
  fi
  test ! -e "$work/getent-$mode.json"
  grep -Fq 'Evidence acquisition failed: manager_account_json' "$work/getent-$mode.err"
done

installed_root="$work/installed-root"
control_root="$work/control-root"
install -D -m755 /usr/bin/true "$installed_root/usr/lib/podmesh-manager/podmesh-managerd"
install -D -m644 "$root/../podmesh-manager.service" "$installed_root/usr/lib/systemd/system/podmesh-manager.service"
mkdir -p "$control_root"
printf '%s\n' '#!/bin/sh' 'set -e' > "$control_root/postinst"
chmod 755 "$control_root/postinst"
installed_binary_sha=$(sha256sum -- "$installed_root/usr/lib/podmesh-manager/podmesh-managerd" | awk '{print $1}')
installed_service_sha=$(sha256sum -- "$installed_root/usr/lib/systemd/system/podmesh-manager.service" | awk '{print $1}')
installed_postinst_sha=$(sha256sum -- "$control_root/postinst" | awk '{print $1}')
jq -n \
  --arg binary_sha256 "$installed_binary_sha" \
  --arg service_sha256 "$installed_service_sha" \
  --arg postinst_sha256 "$installed_postinst_sha" '
  {
    schema_version:"podmesh-manager-candidate-verification/v2",
    verified_at_utc:"2026-09-12T10:01:00Z",
    package:"podmesh-manager",
    version:"0.1",
    architecture:"amd64",
    deb_sha256:"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    binary_sha256:$binary_sha256,
    source_commit:"collector-fixture",
    signed_metadata:{
      inrelease_signature:"verified-by-gpgv-and-pinned-fingerprint",
      signing_fingerprint:"0000000000000000000000000000000000000000",
      keyring_sha256:"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      packages_path:"pool/Packages",
      packages_sha256:"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
      packages_size:123
    },
    regular_payload_files:[
      {path:"/usr/lib/podmesh-manager/podmesh-managerd",sha256:$binary_sha256},
      {path:"/usr/lib/systemd/system/podmesh-manager.service",sha256:$service_sha256}
    ],
    maintainer_scripts:[{name:"postinst",sha256:$postinst_sha256}]
  }
' > "$work/candidate-verification.json"

run_post_install_collector() {
  PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_MANAGER_INSTALLED=1 \
    PODMESH_TEST_CONTROL_ROOT="$control_root" PODMESH_PROC_ROOT="$work/proc" \
    PODMESH_PATH_ROOT="$installed_root" "$root/collect-host.sh" \
    --host-alias lab-a --stage post-install --salt-file "$work/salt" \
    --candidate-verification "$work/candidate-verification.json" --output "$1"
}

run_post_install_collector "$work/post-install.json"
jq -e '
  .schema_version == "podmesh-manager-host-evidence/v5" and
  (.manager.candidate_binding.regular_payload_files_commitment | test("^sha256:[a-f0-9]{64}$")) and
  (.manager.candidate_binding.maintainer_scripts_commitment | test("^sha256:[a-f0-9]{64}$")) and
  (.manager.candidate_binding | has("regular_payload_files") | not) and
  (.manager.candidate_binding | has("maintainer_scripts") | not)
' "$work/post-install.json" >/dev/null

printf '%s\n' 'changed installed service payload' >> "$installed_root/usr/lib/systemd/system/podmesh-manager.service"
if run_post_install_collector "$work/changed-payload.json" >"$work/changed-payload.out" 2>"$work/changed-payload.err"; then
  echo "collector accepted a changed installed regular payload file" >&2
  exit 1
fi
test ! -e "$work/changed-payload.json"
grep -Fq 'Installed package content differs from candidate verification' "$work/changed-payload.err"
install -m644 "$root/../podmesh-manager.service" "$installed_root/usr/lib/systemd/system/podmesh-manager.service"

printf '%s\n' 'changed installed postinst' >> "$control_root/postinst"
if run_post_install_collector "$work/changed-postinst.json" >"$work/changed-postinst.out" 2>"$work/changed-postinst.err"; then
  echo "collector accepted a changed installed postinst" >&2
  exit 1
fi
test ! -e "$work/changed-postinst.json"
grep -Fq 'Installed package content differs from candidate verification' "$work/changed-postinst.err"
printf '%s\n' '#!/bin/sh' 'set -e' > "$control_root/postinst"
chmod 755 "$control_root/postinst"

mkdir -p "$installed_root/etc/podmesh-manager" "$installed_root/var/lib/podmesh-manager"
printf '%s\n' '{"network_enabled":false}' > "$installed_root/etc/podmesh-manager/config.json"
printf '%s\n' 'durable manager state' > "$installed_root/var/lib/podmesh-manager/state.db"
run_upgrade_collector() {
  local stage=$1 output=$2
  shift 2
  env PATH="$work/bin:$PATH" PODMESH_STUB_MODE=success PODMESH_MANAGER_INSTALLED=1 \
    PODMESH_TEST_CONTROL_ROOT="$control_root" PODMESH_PROC_ROOT="$work/proc" \
    PODMESH_PATH_ROOT="$installed_root" "$@" "$root/upgrade/collect-host.sh" \
    --host-alias lab-a --stage "$stage" --salt-file "$work/salt" \
    --candidate-verification "$work/candidate-verification.json" --output "$output"
}

expect_upgrade_collector_failure() {
  local description=$1 expected=$2
  shift 2
  if run_upgrade_collector pre-upgrade "$work/rejected-upgrade.json" "$@" >"$work/rejected-upgrade.out" 2>"$work/rejected-upgrade.err"; then
    echo "upgrade collector accepted $description" >&2
    exit 1
  fi
  test ! -e "$work/rejected-upgrade.json"
  grep -Fq "$expected" "$work/rejected-upgrade.err"
}

run_upgrade_collector pre-upgrade "$work/pre-upgrade.json"
jq -e '
  .schema_version=="podmesh-manager-inactive-upgrade-evidence/v1" and
  .stage=="pre-upgrade" and
  .host_evidence.stage=="post-install" and
  .host_evidence.manager.process_count==0 and
  (.config_commitment|test("^sha256:[a-f0-9]{64}$")) and
  (.state_commitment|test("^sha256:[a-f0-9]{64}$"))
' "$work/pre-upgrade.json" >/dev/null

run_upgrade_collector post-upgrade "$work/post-upgrade.json"
jq -e '.stage=="post-upgrade" and .host_evidence.stage=="post-install"' "$work/post-upgrade.json" >/dev/null

mv -- "$installed_root/etc/podmesh-manager/config.json" "$work/config.json"
ln -s -- "$work/config.json" "$installed_root/etc/podmesh-manager/config.json"
expect_upgrade_collector_failure "a symlinked configuration" "Protected manager configuration must be a regular non-symlink file"
rm -- "$installed_root/etc/podmesh-manager/config.json"
mv -- "$work/config.json" "$installed_root/etc/podmesh-manager/config.json"

mv -- "$installed_root/var/lib/podmesh-manager" "$work/state-directory"
expect_upgrade_collector_failure "a missing state directory" "Manager state path must be a non-symlink directory"
ln -s -- "$work/state-directory" "$installed_root/var/lib/podmesh-manager"
expect_upgrade_collector_failure "a symlinked state directory" "Manager state path must be a non-symlink directory"
rm -- "$installed_root/var/lib/podmesh-manager"
mv -- "$work/state-directory" "$installed_root/var/lib/podmesh-manager"

expect_upgrade_collector_failure "a configuration mutation between commitment passes" \
  "Protected manager configuration or state changed while commitments were collected" \
  PODMESH_COMMIT_COUNTER="$work/config-counter" PODMESH_COMMIT_LABEL=manager-config-v1 \
  PODMESH_MUTATION_TARGET="$installed_root/etc/podmesh-manager/config.json"
printf '%s\n' '{"network_enabled":false}' > "$installed_root/etc/podmesh-manager/config.json"

expect_upgrade_collector_failure "a state mutation between commitment passes" \
  "Protected manager configuration or state changed while commitments were collected" \
  PODMESH_COMMIT_COUNTER="$work/state-counter" PODMESH_COMMIT_LABEL=manager-state-v1 \
  PODMESH_MUTATION_TARGET="$installed_root/var/lib/podmesh-manager/state.db"
printf '%s\n' 'durable manager state' > "$installed_root/var/lib/podmesh-manager/state.db"

expect_upgrade_collector_failure "a host-evidence mutation between base captures" \
  "Host evidence changed while protected-path commitments were collected" \
  PODMESH_SYSTEMCTL_COUNTER="$work/systemctl-counter" PODMESH_MUTATE_HOST_EVIDENCE=1

for mode in failure empty non-array; do
  if PATH="$work/bin:$PATH" PODMESH_STUB_MODE="$mode" PODMESH_PROC_ROOT="$work/proc" "$root/collect-host.sh" --host-alias lab-a --stage pre-install --salt-file "$work/salt" --output "$work/$mode.json" >"$work/$mode.out" 2>"$work/$mode.err"; then
    echo "collector accepted $mode Podman inventory" >&2
    exit 1
  fi
  test ! -e "$work/$mode.json"
  grep -Fq 'Evidence acquisition failed: podman_commitment' "$work/$mode.err"
done

printf '%s\n' 'PASS: collector capture plus installed payload/script binding, tamper, weak-salt, process, account, Podman and inactive-upgrade negative coverage.'
