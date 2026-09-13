#!/bin/bash
set -euo pipefail

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
script="$root/run-default-refusal.sh"
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

bash -n "$script"
mkdir -p "$work/bin" "$work/fs/etc/podmesh-manager" "$work/fs/var/lib/podmesh-manager" "$work/fs/usr/lib/systemd/system" "$work/proc"
printf '%032d' 0 > "$work/salt"
chmod 600 "$work/salt"
printf '%s\n' '[Unit]' > "$work/fs/usr/lib/systemd/system/podmesh-manager.service"
cat > "$work/fs/etc/podmesh-manager/config.json" <<'JSON'
{"network":{"replica_id":"00000000-0000-4000-8000-000000000001","database_path":"/var/lib/podmesh-manager/manager.sqlite","manager":{"logical_manager_id":"00000000-0000-4000-8000-000000000010","replicas":[{"replica_id":"00000000-0000-4000-8000-000000000001","host_id":"00000000-0000-4000-8000-000000000101"},{"replica_id":"00000000-0000-4000-8000-000000000002","host_id":"00000000-0000-4000-8000-000000000102"}],"grants":[{"scope":"scope-b","owner_replica_id":"00000000-0000-4000-8000-000000000002"},{"scope":"scope-a","owner_replica_id":"00000000-0000-4000-8000-000000000001"}]},"bind":"127.0.0.1:9443","peers":[{"replica_id":"00000000-0000-4000-8000-000000000002","endpoint":"10.0.0.2:9443","shared_key_hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]},"control_socket":"/run/podmesh-manager/control.sock","observation_writer_uid":0,"interval_ms":1000,"max_backoff_ms":30000,"incoming_workers":1}
JSON
chmod 640 "$work/fs/etc/podmesh-manager/config.json"

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
if [ "${1-}" = -c ] && [ "${2-}" = '%F|%u|%g|%a|%s' ]; then
  case "${4-}" in
    */etc/podmesh-manager/config.json) printf 'regular file|0|124|640|1\n' ;;
    */var/lib/podmesh-manager) printf 'directory|123|124|750|1\n' ;;
    */run/podmesh-manager) printf 'directory|123|124|700|1\n' ;;
    */usr/lib/systemd/system/podmesh-manager.service) printf 'regular file|0|0|644|1\n' ;;
    *) exec /usr/bin/stat "$@" ;;
  esac
  exit 0
fi
exec /usr/bin/stat "$@"
EOF
cat > "$work/bin/getent" <<'EOF'
#!/bin/sh
case "${1-}" in
  passwd) printf '%s\n' 'podmesh-manager:x:123:124::/nonexistent:/usr/sbin/nologin' ;;
  group) printf '%s\n' 'podmesh-manager:x:124:' ;;
esac
EOF
cat > "$work/bin/install" <<'EOF'
#!/bin/sh
for path in "$@"; do :; done
mkdir -p "$path"
chmod 700 "$path"
EOF
cat > "$work/bin/runuser" <<'EOF'
#!/bin/sh
shift 2
[ "${1-}" = -- ] && shift
if [ "${1-}" = id ]; then printf '%s\n' 123; exit 0; fi
shift
printf '%s\n' '{"configuration_valid":true,"durable_store_checked":false,"network_started":false}'
EOF
cat > "$work/bin/systemctl" <<'EOF'
#!/bin/sh
if [ "${1-}" = start ]; then exit 1; fi
case "${2-}" in
  podmesh-manager.service)
    if [ "${PODMESH_TEST_POST-}" = 1 ]; then
      cat <<OUT
LoadState=loaded
ActiveState=failed
SubState=failed
UnitFileState=disabled
MainPID=0
ExecMainPID=41
ExecMainStatus=1
Result=exit-code
InvocationID=11111111111111111111111111111111
ExecMainStartTimestampMonotonic=100
NRestarts=0
FragmentPath=/usr/lib/systemd/system/podmesh-manager.service
DropInPaths=\${PODMESH_TEST_DROPIN-}
OUT
    else
      export PODMESH_TEST_POST=1
      cat <<OUT
LoadState=loaded
ActiveState=inactive
SubState=dead
UnitFileState=disabled
MainPID=0
ExecMainPID=0
ExecMainStatus=0
Result=success
InvocationID=
ExecMainStartTimestampMonotonic=0
NRestarts=0
FragmentPath=/usr/lib/systemd/system/podmesh-manager.service
DropInPaths=\${PODMESH_TEST_DROPIN-}
OUT
    fi
    ;;
  podmesh.service|podmesh-web-observer.service)
    cat <<OUT
LoadState=loaded
ActiveState=active
SubState=running
UnitFileState=enabled
MainPID=33
ExecMainPID=33
InvocationID=22222222222222222222222222222222
ExecMainStartTimestampMonotonic=42
NRestarts=0
OUT
    ;;
esac
EOF
cat > "$work/bin/journalctl" <<'EOF'
#!/bin/sh
printf '%s\n' '2026-09-12T00:00:00+00:00 runtime networking disabled; set PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers explicitly'
EOF
cat > "$work/bin/podman" <<'EOF'
#!/bin/sh
case "${3-}" in
  -aq) exit 0 ;;
esac
exit 0
EOF
cat > "$work/bin/ss" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$work/bin/ip" <<'EOF'
#!/bin/sh
printf '%s\n' '[]'
EOF
cat > "$work/bin/nft" <<'EOF'
#!/bin/sh
printf '%s\n' 'table inet filter {}'
EOF
for file in "$work/bin"/*; do chmod +x "$file"; done

# The process-global export makes the systemctl stub observe the refusal only
# after start.  A small wrapper persists it across invocations.
cat > "$work/bin/systemctl" <<EOF
#!/bin/sh
state="$work/systemctl-state"
if [ "\${1-}" = start ]; then touch "\$state"; exit 1; fi
case "\${2-}" in
  podmesh-manager.service)
    if [ -e "\$state" ]; then active=failed; sub=failed; result=exit-code; status=1; execpid=41; invocation=11111111111111111111111111111111; start=100; else active=inactive; sub=dead; result=success; status=0; execpid=0; invocation=; start=0; fi
    cat <<OUT
LoadState=loaded
ActiveState=\$active
SubState=\$sub
UnitFileState=disabled
MainPID=0
ExecMainPID=\$execpid
ExecMainStatus=\$status
Result=\$result
InvocationID=\$invocation
ExecMainStartTimestampMonotonic=\$start
NRestarts=0
FragmentPath=/usr/lib/systemd/system/podmesh-manager.service
DropInPaths=\${PODMESH_TEST_DROPIN-}
OUT
    ;;
  podmesh.service|podmesh-web-observer.service)
    cat <<OUT
LoadState=loaded
ActiveState=active
SubState=running
UnitFileState=enabled
MainPID=33
ExecMainPID=33
InvocationID=22222222222222222222222222222222
ExecMainStartTimestampMonotonic=42
NRestarts=0
OUT
    ;;
esac
EOF
chmod +x "$work/bin/systemctl"

PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/evidence.json"
jq -e '
  .schema_version=="podmesh-manager-default-refusal-evidence/v1" and
  .offline_validation.effective_uid==123 and .offline_validation.runtime.created and .offline_validation.runtime.removed and
  .offline_validation.runtime.metadata.uid==123 and .offline_validation.runtime.metadata.mode=="700" and
  .default_refusal.post_unit.active_state=="failed" and (.default_refusal.post_unit.invocation_id|test("^sha256:")) and
  .default_refusal.journal.network_disabled_refusal_observed and
  (.before.podman_containers_commitment|test("^sha256:")) and
  .after.manager.process_count==0 and .assertions.podman_containers_unchanged and
  .after.firewall.status=="available-successful" and .assertions.firewall_unchanged==true
' "$work/evidence.json" >/dev/null

# Low-entropy endpoints must not appear in clear text, and their commitments
# must use the private salt rather than unsalted SHA-256.
if grep -Fq '10.0.0.2:9443' "$work/evidence.json"; then
  echo 'endpoint leaked into refusal evidence' >&2
  exit 1
fi
endpoint_commitment=$(jq -r '.configuration.commitments.peers[0].endpoint_commitment' "$work/evidence.json")
unsalted="sha256:$(printf %s '10.0.0.2:9443' | sha256sum | awk '{print $1}')"
[ "$endpoint_commitment" != "$unsalted" ] || { echo 'endpoint commitment is unsalted' >&2; exit 1; }
jq -e '.configuration.commitments.observation_writer_uid==0' "$work/evidence.json" >/dev/null

# Replica and grant ordering is presentation-only.  The topology commitment is
# canonical, while a changed grant remains visible as a changed commitment.
config_fixture="$work/fs/etc/podmesh-manager/config.json"
cp -- "$config_fixture" "$work/config-pristine.json"
base_topology=$(jq -r '.configuration.commitments.topology_commitment' "$work/evidence.json")
jq '.network.manager.replicas |= reverse | .network.manager.grants |= reverse' "$work/config-pristine.json" > "$work/config-reordered.json"
cp -- "$work/config-reordered.json" "$config_fixture"
rm -f "$work/systemctl-state"
PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/reordered-evidence.json"
[ "$(jq -r '.configuration.commitments.topology_commitment' "$work/reordered-evidence.json")" = "$base_topology" ] || { echo 'topology commitment depends on replica or grant ordering' >&2; exit 1; }
jq '.network.manager.grants[0].owner_replica_id = "00000000-0000-4000-8000-000000000001"' "$work/config-pristine.json" > "$work/config-grant-drift.json"
cp -- "$work/config-grant-drift.json" "$config_fixture"
rm -f "$work/systemctl-state"
PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/grant-drift-evidence.json"
[ "$(jq -r '.configuration.commitments.topology_commitment' "$work/grant-drift-evidence.json")" != "$base_topology" ] || { echo 'topology commitment does not cover scope grants' >&2; exit 1; }
cp -- "$work/config-pristine.json" "$config_fixture"

# A successful-but-empty firewall command is still observable as success.  A
# command failure becomes explicit unknown rather than an empty-input hash.
cat > "$work/bin/nft" <<'EOF'
#!/bin/sh
exit 73
EOF
chmod +x "$work/bin/nft"
rm -f "$work/systemctl-state"
PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/unknown-firewall.json"
jq -e '.before.firewall.status=="unknown" and .before.firewall.commitment==null and .assertions.firewall_unchanged==null' "$work/unknown-firewall.json" >/dev/null

expect_harness_failure() {
  if "$@" >"$work/negative.out" 2>"$work/negative.err"; then
    echo 'default-refusal harness unexpectedly accepted a negative case' >&2
    exit 1
  fi
}

# The writer identity is mandatory evidence, even though the three-host
# comparator is responsible for enforcing the deployment policy value zero.
jq 'del(.observation_writer_uid)' "$work/config-pristine.json" > "$work/config-no-writer.json"
cp -- "$work/config-no-writer.json" "$config_fixture"
rm -f "$work/systemctl-state"
expect_harness_failure env PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/no-writer.json"
cp -- "$work/config-pristine.json" "$config_fixture"

# A systemd drop-in invalidates the default-unit claim before the start attempt.
mkdir -p "$work/fs/usr/lib/systemd/system/podmesh-manager.service.d"
printf '%s\n' '[Service]' > "$work/fs/usr/lib/systemd/system/podmesh-manager.service.d/test.conf"
rm -f "$work/systemctl-state"
expect_harness_failure env PODMESH_TEST_DROPIN=/usr/lib/systemd/system/podmesh-manager.service.d/test.conf PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/dropin.json"
rm -rf "$work/fs/usr/lib/systemd/system/podmesh-manager.service.d"

# Successful observations that differ before and after the attempt must fail,
# rather than being reduced to a PASS record with a false assertion.
cat > "$work/bin/ip" <<EOF
#!/bin/sh
state="$work/ip-state"
if [ -e "\$state" ]; then printf '%s\n' '[{"dst":"198.51.100.0/24"}]'; else touch "\$state"; printf '%s\n' '[]'; fi
EOF
chmod +x "$work/bin/ip"
rm -f "$work/systemctl-state" "$work/ip-state"
expect_harness_failure env PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/route-change.json"

cat > "$work/bin/ip" <<'EOF'
#!/bin/sh
printf '%s\n' '[]'
EOF
cat > "$work/bin/nft" <<EOF
#!/bin/sh
state="$work/nft-state"
if [ -e "\$state" ]; then printf '%s\n' 'table inet changed {}'; else touch "\$state"; printf '%s\n' 'table inet filter {}'; fi
EOF
chmod +x "$work/bin/ip" "$work/bin/nft"
rm -f "$work/systemctl-state" "$work/nft-state"
expect_harness_failure env PATH="$work/bin:$PATH" PODMESH_PATH_ROOT="$work/fs" PODMESH_PROC_ROOT="$work/proc" PODMESH_RUNUSER_BIN="$work/bin/runuser" PODMESH_MANAGER_BINARY=/manager-stub "$script" --host-alias lab-a --salt-file "$work/salt" --output "$work/firewall-change.json"

printf '%s\n' 'PASS: default refusal harness proves protected offline validation, InvocationID-bound refusal, salted endpoint evidence, stable existing-service and Podman commitments, and explicit firewall unknown states.'
"$root/tests/compare-three-hosts-tests.sh"
