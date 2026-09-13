#!/bin/bash
set -euo pipefail
repo=$(cd -- "$(dirname -- "$0")/.." && pwd)
for command in sudo bwrap; do command -v "$command" >/dev/null || { echo "SKIP: $command is unavailable"; exit 0; }; done
sudo -n true >/dev/null 2>&1 || { echo "SKIP: passwordless sudo is unavailable"; exit 0; }
w=$(mktemp -d)
trap 'sudo rm -rf "$w"' EXIT
mkdir -p "$w"/{etc,state,root,run/lock,usr,fakebin}
cat > "$w/etc/config.json" <<'EOF'
{"network":{"replica_id":"replica-a","database_path":"/var/lib/podmesh-manager/manager.sqlite","manager":{"logical_manager_id":"logical","replicas":[{"replica_id":"replica-a","host_id":"host-a"},{"replica_id":"replica-b","host_id":"host-b"},{"replica_id":"replica-c","host_id":"host-c"}],"grants":[]},"bind":"127.0.0.1:9443","peers":[{"replica_id":"replica-b","endpoint":"192.0.2.2:9443","shared_key_hex":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"replica_id":"replica-c","endpoint":"192.0.2.3:9443","shared_key_hex":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]},"control_socket":"/run/podmesh-manager/control.sock","observation_writer_uid":0,"interval_ms":1000,"max_backoff_ms":30000,"incoming_workers":1}
EOF
cat > "$w/root/map.json" <<'EOF'
{"schema_version":"podmesh-manager-alias-replica-map/v1","aliases":{"lab-a":"replica-a","lab-b":"replica-b","lab-c":"replica-c"}}
EOF
head -c 32 /dev/zero > "$w/root/salt"
cat > "$w/usr/podmesh-managerd" <<'EOF'
#!/bin/sh
printf '%s\n' '{"configuration_valid":true,"durable_store_checked":false,"network_started":false}'
EOF
chmod 755 "$w/usr/podmesh-managerd"
hash=$(sha256sum "$w/usr/podmesh-managerd"|awk '{print $1}')
printf '{"schema_version":"podmesh-manager-candidate-verification/v2","package":"podmesh-manager","version":"0.1.0-test","binary_sha256":"%s"}\n' "$hash" > "$w/root/report.json"
cat > "$w/fakebin/systemctl" <<'EOF'
#!/bin/sh
cat <<'STATE'
LoadState=loaded
ActiveState=inactive
SubState=dead
UnitFileState=disabled
MainPID=0
InvocationID=old-refusal-only
NRestarts=0
ExecMainStartTimestampMonotonic=1
ExecMainExitTimestampMonotonic=2
STATE
EOF
cat > "$w/fakebin/getent" <<'EOF'
#!/bin/sh
case "$1:$2" in passwd:podmesh-manager) echo 'podmesh-manager:x:995:995::/nonexistent:/usr/sbin/nologin';; group:podmesh-manager) echo 'podmesh-manager:x:995:';; *) exit 2;; esac
EOF
cat > "$w/fakebin/dpkg-query" <<'EOF'
#!/bin/sh
case "$*" in *Status-Status*) printf installed;; *) printf 0.1.0-test;; esac
EOF
cat > "$w/fakebin/dpkg" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$w/fakebin/ss" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$w/fakebin/runuser" <<'EOF'
#!/bin/sh
shift 3
exec "$@"
EOF
cat > "$w/fakebin/install" <<'EOF'
#!/bin/bash
args=()
while [ "$#" -gt 0 ]; do case "$1" in -o|-g) key=$1; val=$2; [ "$val" != podmesh-manager ] || val=995; [ "$val" != root ] || val=0; args+=("$key" "$val"); shift 2;; *) args+=("$1"); shift;; esac; done
exec /usr/bin/install "${args[@]}"
EOF
cat > "$w/fakebin/chown" <<'EOF'
#!/bin/bash
owner=$1; shift
owner=${owner//podmesh-manager/995}; owner=${owner//root/0}
exec /usr/bin/chown "$owner" "$@"
EOF
chmod 755 "$w/fakebin"/*
sudo chown 0:995 "$w/etc/config.json"; sudo chown 995:995 "$w/state"; sudo chmod 640 "$w/etc/config.json"; sudo chmod 750 "$w/state"
sudo chmod 600 "$w/root"/*; sudo chown 0:0 "$w/root" "$w/root"/* "$w/run" "$w/run/lock"; sudo chmod 700 "$w/root" "$w/run"
cat > "$w/inside.sh" <<EOF
#!/bin/bash
set -euo pipefail
export PATH=$w/fakebin:/usr/bin:/bin
cmd=("$repo/transition-host.sh" --host-alias lab-a --mapping /root/map.json --salt-file /root/salt --candidate-verification /root/report.json --backup-file /root/backup.json --evidence-directory /root/evidence)
"\${cmd[@]}" --mode apply
test "\$(jq '.network.manager.grants|length' /etc/podmesh-manager/config.json)" = 3
# The first-store-open transition's evidence must not have grown the operational transition's fields.
jq -e '.schema_version=="podmesh-manager-config-transition-evidence/v1" and (has("transition_kind")|not) and (.transition|has("state_listing_commitment")|not) and (.transition|has("changed_keys")|not) and (.preconditions|has("state_directory_listing_unchanged")|not) and .preconditions.state_directory_empty==true' /root/evidence/config-transition-result.json >/dev/null
cp /etc/podmesh-manager/config.json /root/applied.json
# Simulate interruption after the prepared ledger but before backup/config replacement.
cp /root/backup.json /etc/podmesh-manager/config.json
rm /root/backup.json /root/evidence/config-transition-result.json*
jq '.state="prepared"' /root/evidence/config-transition-ledger.json > /root/evidence/.ledger && mv /root/evidence/.ledger /root/evidence/config-transition-ledger.json
chmod 600 /root/evidence/config-transition-ledger.json
"\${cmd[@]}" --mode apply
test "\$(jq '.network.manager.grants|length' /etc/podmesh-manager/config.json)" = 3
"\${cmd[@]}" --mode apply
"\${cmd[@]}" --mode rollback
test "\$(jq '.network.manager.grants|length' /etc/podmesh-manager/config.json)" = 0
rm /root/evidence/config-transition-result.json*
"\${cmd[@]}" --mode rollback
test -f /root/evidence/config-transition-result.json.sha256
jq -e '.action=="rolled-back" and .result=="PASS"' /root/evidence/config-transition-result.json >/dev/null
# A separate transition proves that sealing permanently closes rollback.
cmd2=("$repo/transition-host.sh" --host-alias lab-a --mapping /root/map.json --salt-file /root/salt --candidate-verification /root/report.json --backup-file /root/backup2.json --evidence-directory /root/evidence2)
"\${cmd2[@]}" --mode apply
"\${cmd2[@]}" --mode seal-for-activation
jq -e '.state=="sealed-for-activation" and .rollback_window_open==false' /root/evidence2/config-transition-ledger.json >/dev/null
jq -e '.schema_version=="podmesh-manager-rollback-window-closure/v1" and .result=="PASS" and .state_directory_empty_at_closure==true and .manager_inactive_at_closure==true and (has("transition_kind")|not)' /root/evidence2/rollback-window-closed.json >/dev/null
if "\${cmd2[@]}" --mode rollback >/dev/null 2>&1; then echo 'sealed transition accepted rollback' >&2; exit 1; fi
# The operational transition, after activation: durable state and the boundary marker exist, the three grants are in place.
: > /var/lib/podmesh-manager/manager.sqlite; chown 995:995 /var/lib/podmesh-manager/manager.sqlite; chmod 600 /var/lib/podmesh-manager/manager.sqlite
printf '%s\n' '{"schema_version":"podmesh-manager-activation-boundary/v1","host_alias":"lab-a","package_version":"0.1.0-test","binary_sha256":"$hash","rollback_window":"closed"}' > /etc/podmesh-manager/.manager2-activation-started
chmod 600 /etc/podmesh-manager/.manager2-activation-started
cmd3g=("$repo/transition-host.sh" --host-alias lab-a --mapping /root/map.json --salt-file /root/salt --candidate-verification /root/report.json --backup-file /root/backup3.json --evidence-directory /root/evidence3)
if "\${cmd3g[@]}" --mode apply >/dev/null 2>&1; then echo 'first-store-open transition accepted durable state and a boundary marker' >&2; exit 1; fi
test ! -e /root/backup3.json
cmd3=("$repo/transition-host.sh" --transition incoming-workers --host-alias lab-a --mapping /root/map.json --salt-file /root/salt --candidate-verification /root/report.json --backup-file /root/backup-workers.json --evidence-directory /root/evidence-workers)
"\${cmd3[@]}" --mode apply
test "\$(jq '.incoming_workers' /etc/podmesh-manager/config.json)" = 2
test "\$(jq '.network.manager.grants|length' /etc/podmesh-manager/config.json)" = 3
jq -e '.schema_version=="podmesh-manager-config-transition-evidence/v2" and .transition_kind=="incoming-workers" and .action=="applied" and .transition.changed_keys==["incoming_workers"] and .transition.incoming_workers=={"from":1,"to":2} and .transition.grant_count==3 and .preconditions.durable_state_present==true and .preconditions.activation_marker_present==true and (.preconditions|has("state_directory_empty")|not) and .rollback_permitted_only_while=={"activation_markers_unchanged":true}' /root/evidence-workers/config-transition-result.json >/dev/null
jq -e '.transition_kind=="incoming-workers" and .state=="applied"' /root/evidence-workers/config-transition-ledger.json >/dev/null
test -e /var/lib/podmesh-manager/manager.sqlite
"\${cmd3[@]}" --mode apply
if "\${cmd3g[@]}" --backup-file /root/backup-workers.json --evidence-directory /root/evidence-workers --mode rollback >/dev/null 2>&1; then echo 'a three-grant rollback accepted the operational ledger' >&2; exit 1; fi
# That refusal lands on the state-directory precondition, not on the ledger kind. Prove the kind check itself
# by pointing the operational transition at the sealed three-grants ledger, which reaches read_ledger.
if foreign=\$("$repo/transition-host.sh" --transition incoming-workers --host-alias lab-a --mapping /root/map.json --salt-file /root/salt --candidate-verification /root/report.json --backup-file /root/backup2.json --evidence-directory /root/evidence2 --mode rollback 2>&1); then
  echo 'the operational transition accepted a three-grants ledger' >&2; exit 1
fi
case "\$foreign" in *'ledger transition kind mismatch'*) ;; *) echo "a foreign ledger kind was refused for another reason: \$foreign" >&2; exit 1;; esac
"\${cmd3[@]}" --mode rollback
test "\$(jq '.incoming_workers' /etc/podmesh-manager/config.json)" = 1
jq -e '.action=="rolled-back" and .transition_kind=="incoming-workers"' /root/evidence-workers/config-transition-result.json >/dev/null
cmd4=("$repo/transition-host.sh" --transition incoming-workers --host-alias lab-a --mapping /root/map.json --salt-file /root/salt --candidate-verification /root/report.json --backup-file /root/backup-workers2.json --evidence-directory /root/evidence-workers2)
"\${cmd4[@]}" --mode apply
"\${cmd4[@]}" --mode seal-for-activation
jq -e '.state=="sealed-for-activation" and .transition_kind=="incoming-workers" and .rollback_window_open==false' /root/evidence-workers2/config-transition-ledger.json >/dev/null
jq -e '.schema_version=="podmesh-manager-rollback-window-closure/v2" and .result=="PASS" and .transition_kind=="incoming-workers" and (has("state_directory_empty_at_closure")|not) and .durable_state_present_at_closure==true and .activation_marker_present_at_closure==true and .manager_inactive_at_closure==true' /root/evidence-workers2/rollback-window-closed.json >/dev/null
if "\${cmd4[@]}" --mode rollback >/dev/null 2>&1; then echo 'sealed operational transition accepted rollback' >&2; exit 1; fi
test "\$(jq '.incoming_workers' /etc/podmesh-manager/config.json)" = 2
EOF
chmod 755 "$w/inside.sh"
sudo bwrap --bind / / --dev-bind /dev /dev --bind "$w/etc" /etc/podmesh-manager --bind "$w/state" /var/lib/podmesh-manager --bind "$w/root" /root --bind "$w/run" /run --bind "$w/usr" /usr/lib/podmesh-manager -- "$w/inside.sh"
echo 'PASS: sandboxed host apply, prepared recovery, idempotent apply, rollback, rollback-evidence repair, and the operational transition after activation.'
