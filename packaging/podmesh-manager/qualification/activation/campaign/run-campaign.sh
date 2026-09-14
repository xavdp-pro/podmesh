#!/bin/bash
# Workstation driver for a manager2 G2 live-activation campaign: the repeatable evidence-v3 capture (first run as campaign 6).
# `campaign` runs every phase in order and stops at the first failure, rolling back whatever it activated;
# each phase is also invocable alone. Every remote command and its raw output is appended to campaign.log
# (private: it names hosts). Public evidence is only what the harness writes under live-evidence/ plus
# campaign-summary.json, which carries aliases, hashes, counts and verdicts and never an address or a key.
set -euo pipefail
export LC_ALL=C
W=${PODMESH_CAMPAIGN_DIR:?set PODMESH_CAMPAIGN_DIR to the campaign directory, identical on hosts and workstation}
K=${PODMESH_CAMPAIGN_KNOWN_HOSTS:?set PODMESH_CAMPAIGN_KNOWN_HOSTS to the private known_hosts file}
PLAN=$W/campaign-plan.json
LOG=$W/campaign.log
SUMMARY=$W/campaign-summary.json
R=${PODMESH_CAMPAIGN_REMOTE_DIR:?set PODMESH_CAMPAIGN_REMOTE_DIR to the private material directory on each host}   # salt, report, harness, drop-in
A=$R/${PODMESH_CAMPAIGN_HARNESS:-harness6}/packaging/podmesh-manager/qualification/activation
E=$W/live-evidence                                       # same absolute path on host and workstation: sidecars stay valid
COMPARATOR=$(cd -- "$(dirname -- "$0")/.." && pwd)/compare-evidence.py
CANDIDATE=${PODMESH_CAMPAIGN_CANDIDATE:?set PODMESH_CAMPAIGN_CANDIDATE to the frozen candidate binary on the workstation}

# Aliases resolve through a private hosts file kept outside Git: {"lab-a": "user@address", ...}.
host_of() { jq -er --arg a "$1" '.[$a]' "${PODMESH_CAMPAIGN_HOSTS:?set PODMESH_CAMPAIGN_HOSTS to the private alias-to-target file}" 2>/dev/null || { echo "unknown alias $1" >&2; exit 2; }; }
remote() { # remote <alias> <<'EOF' script EOF  — runs as root, logs everything
  local alias=$1 host; host=$(host_of "$alias"); local script; script=$(cat)
  { printf '\n===== %s  %s  phase=%s =====\n' "$(date -u +%FT%TZ)" "$alias" "${PHASE:-?}"; printf '%s\n' "$script" | sed 's/^/> /'; } >> "$LOG"
  local out rc=0
  out=$(ssh -o BatchMode=yes -o ConnectTimeout=8 -o UserKnownHostsFile="$K" "lab@$host" 'sudo -n bash -s' <<<"$script" 2>&1) || rc=$?
  printf '%s\n[exit=%s]\n' "$out" "$rc" >> "$LOG"
  printf '%s\n' "$out"; [ "$rc" -eq 0 ] || { echo "[$alias] exit=$rc" >&2; return "$rc"; }
}
op_id() { jq -er --arg a "$1" '.operation_ids[$a]' "$PLAN"; }
value() { jq -er .observation_value "$PLAN"; }
INSPECT_JQ='{h:.logical_history_sha256[0:16],history:.history_count,receipts:.receipt_count,audit:.audit_event_count,incomplete:(.incomplete_attempts|length),integrity:.sqlite_integrity_result,conflicts:(.conflicts|length),blocked:(.blocked_exclusive_resources|length)}'

PHASE=${1:-}; shift || true
case "$PHASE" in
prepare-dirs)  # root 0700 parents on every host; refuses an evidence directory that already exists
  for a in lab-a lab-b lab-c; do remote "$a" <<EOF
set -euo pipefail; umask 077
install -d -m 0700 -o root -g root -- $W $E
[ ! -e $E/$a ] && echo "$a: $E/$a absent (fresh)" || { echo "$a: $E/$a already exists"; exit 1; }
systemctl is-active podmesh-manager.service >/dev/null 2>&1 && { echo "$a: manager is active before the campaign"; exit 1; }
[ -z "\$(systemctl show podmesh-manager.service -p DropInPaths --value)" ] || { echo "$a: a drop-in is installed before the campaign"; exit 1; }
sha256sum $A/capture-host.sh $A/activate-host.sh $A/append-observation.py $A/wait-ready.py $A/graceful-shutdown.py $A/validate-dropin.py | awk '{print "harness", \$1, \$2}' | sed "s|$A/||"
EOF
  done;;
activate)  # activate <alias>: pre-activation capture, drop-in, start, readiness, active-baseline capture
  a=$1; remote "$a" <<EOF
set -uo pipefail; export LC_ALL=C
$A/activate-host.sh --host-alias $a --salt-file $R/salt --candidate-verification $R/candidate-verification.json --dropin-source $R/90-g2-network.conf --evidence-directory $E/$a; rc=\$?
echo "activate exit=\$rc"
echo "ledger:    \$(jq -c '{state,package_version}' $E/$a/activation-ledger.json)"
echo "service:   \$(jq -c '.service|{active_state,sub_state,unit_file_state,main_pid,n_restarts}' $E/$a/active-baseline.json)"
echo "inspect:   \$(jq -c '.inspection|{history_count,receipt_count,incomplete_attempt_count,sqlite_integrity_result,h:.logical_history_sha256[0:16]}' $E/$a/active-baseline.json)"
echo "sidecars:  \$(cd $E/$a && sha256sum -c pre-activation.json.sha256 active-baseline.json.sha256 2>&1 | tr '\n' ' ')"
exit \$rc
EOF
  ;;
observe)  # observe <alias>: one owned, non-exclusive observation through the local socket
  a=$1; remote "$a" <<EOF
set -euo pipefail; export LC_ALL=C
$A/append-observation.py --operation-id $(op_id "$a") --scope g2/$a/observations --subject campaign-probe --value $(value)
EOF
  ;;
inspect)  # read-only canonical inspection on every host
  for a in lab-a lab-b lab-c; do printf '%s: ' "$a"; remote "$a" <<EOF
set -euo pipefail; export LC_ALL=C
runuser -u podmesh-manager -- /usr/lib/podmesh-manager/podmesh-managerd --inspect-store --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager | jq -c '$INSPECT_JQ'
EOF
  done;;
wait-converged)  # poll until the three canonical digests agree and the history grew by exactly three
  polls=$(jq -r .convergence_wait.max_polls "$PLAN"); every=$(jq -r .convergence_wait.poll_seconds "$PLAN")
  base=$(jq -r '.inspection.history_count' "$E/lab-a/active-baseline.json" 2>/dev/null || ssh -o BatchMode=yes -o UserKnownHostsFile="$K" "lab@$(host_of lab-a)" "sudo -n jq -r .inspection.history_count $E/lab-a/active-baseline.json")
  for i in $(seq 1 "$polls"); do
    lines=$("$0" inspect 2>/dev/null | grep -v "^\[" || true)
    hs=$(printf '%s\n' "$lines" | sed 's/^[a-z-]*: //' | jq -r .h | sort -u | wc -l)
    hist=$(printf '%s\n' "$lines" | sed 's/^[a-z-]*: //' | jq -r .history | sort -u)
    echo "poll $i: distinct digests=$hs history=$(echo $hist | tr '\n' ' ') (baseline $base)"
    if [ "$hs" = 1 ] && [ "$(echo "$hist" | wc -l)" = 1 ] && [ "$hist" = "$((base + 3))" ]; then echo "converged after $i polls"; exit 0; fi
    sleep "$every"
  done
  echo "not converged after $polls polls" >&2; exit 4;;
converged)  # converged <alias>: separate capture with inspection, then seal it into the ledger
  a=$1; remote "$a" <<EOF
set -euo pipefail; export LC_ALL=C
$A/capture-host.sh --host-alias $a --stage converged --salt-file $R/salt --candidate-verification $R/candidate-verification.json --output $E/$a/converged.json --with-inspection
echo "converged: \$(jq -c '.inspection|{history_count,receipt_count,incomplete_attempt_count,h:.logical_history_sha256[0:16]}' $E/$a/converged.json)  same_pid_as_baseline=\$([ "\$(jq .service.main_pid $E/$a/converged.json)" = "\$(jq .service.main_pid $E/$a/active-baseline.json)" ] && echo yes || echo NO)"
$A/activate-host.sh --mode seal-converged --host-alias $a --salt-file $R/salt --candidate-verification $R/candidate-verification.json --evidence-directory $E/$a
echo "ledger:    \$(jq -c '{state}' $E/$a/activation-ledger.json)"
EOF
  ;;
rollback)  # rollback <alias>: typed graceful shutdown, hash-bound drop-in removal, post-cleanup capture
  a=$1; remote "$a" <<EOF
set -uo pipefail; export LC_ALL=C
$A/activate-host.sh --mode rollback --host-alias $a --salt-file $R/salt --candidate-verification $R/candidate-verification.json --evidence-directory $E/$a; rc=\$?
echo "rollback exit=\$rc"
echo "ledger:    \$(jq -c '{state,cleanup_restart}' $E/$a/activation-ledger.json)"
echo "shutdown:  \$(jq -c '{typed:.typed_request_acknowledged,exited:.process_exited_successfully,forced:.forced_signal_used}' $E/$a/graceful-shutdown.json 2>/dev/null || echo absent)"
echo "service:   \$(jq -c '.service|{active_state,unit_file_state,result,exec_main_status}' $E/$a/post-cleanup.json 2>/dev/null || echo 'post-cleanup absent')"
echo "inspect:   \$(jq -c '.inspection|{history_count,incomplete_attempt_count,h:.logical_history_sha256[0:16]}' $E/$a/post-cleanup.json 2>/dev/null || true)"
echo "dropin:    \$(systemctl show podmesh-manager.service -p DropInPaths --value | sed 's/^$/none/')  unit: \$(systemctl show podmesh-manager.service -p ActiveState -p UnitFileState --value | tr '\n' ' ')"
exit \$rc
EOF
  ;;
preserve)  # preserve <alias>: copy the raw store after the manager stopped, and derive an inspection from the copy
  a=$1; remote "$a" <<EOF
set -euo pipefail; export LC_ALL=C; umask 077
systemctl is-active podmesh-manager.service >/dev/null 2>&1 && { echo "manager still active; a store is preserved only at rest"; exit 3; }
d=$E/$a/preserved-store; install -d -m 0700 -o root -g root -- \$d
for f in manager.sqlite manager.sqlite-wal manager.sqlite-shm; do [ -e /var/lib/podmesh-manager/\$f ] && cp -p -- /var/lib/podmesh-manager/\$f \$d/\$f; done
(cd \$d && sha256sum manager.sqlite* > SHA256SUMS)
# The candidate refuses a database that is not a direct child of the declared state directory, so the
# copy is inspected under a private configuration rebuilt from the installed one with only that path
# changed; the configuration copy holds peer material and lives only inside the removed temporary directory.
w=\$(mktemp -d); cp -- \$d/manager.sqlite* \$w/
jq --arg p "\$w/manager.sqlite" '.network.database_path=\$p' /etc/podmesh-manager/config.json > \$w/config.json
chown -R podmesh-manager:podmesh-manager \$w; chmod 700 \$w; chmod 600 \$w/config.json
runuser -u podmesh-manager -- /usr/lib/podmesh-manager/podmesh-managerd --inspect-store --config \$w/config.json --state-dir \$w > $E/$a/derived-inspection.json
rm -rf -- \$w
sha256sum $E/$a/derived-inspection.json > $E/$a/derived-inspection.json.sha256
echo "store:     \$(cat \$d/SHA256SUMS | cut -c1-16 | tr '\n' ' ') bytes=\$(stat -c %s \$d/manager.sqlite)"
echo "derived:   \$(jq -c '$INSPECT_JQ' $E/$a/derived-inspection.json)"
EOF
  ;;
fetch)  # copy the evidence, sidecars, preserved stores and derived inspections to the identical local path, then verify
  for a in lab-a lab-b lab-c; do
    mkdir -p "$E/$a/preserved-store"; chmod 700 "$E/$a" "$E/$a/preserved-store"
    for f in pre-activation.json pre-activation.json.sha256 active-baseline.json active-baseline.json.sha256 converged.json converged.json.sha256 post-cleanup.json post-cleanup.json.sha256 activation-ledger.json derived-inspection.json derived-inspection.json.sha256 preserved-store/SHA256SUMS preserved-store/manager.sqlite; do
      ssh -o BatchMode=yes -o UserKnownHostsFile="$K" "lab@$(host_of "$a")" "sudo -n cat $E/$a/$f" > "$E/$a/$f"
    done
    for f in manager.sqlite-wal manager.sqlite-shm; do ssh -o BatchMode=yes -o UserKnownHostsFile="$K" "lab@$(host_of "$a")" "sudo -n cat $E/$a/preserved-store/$f 2>/dev/null" > "$E/$a/preserved-store/$f" || true; [ -s "$E/$a/preserved-store/$f" ] || rm -f "$E/$a/preserved-store/$f"; done
    (cd "$E/$a" && sha256sum -c -- *.json.sha256 && cd preserved-store && sha256sum -c SHA256SUMS)
  done;;
compare)
  "$COMPARATOR" --phase three-host \
    --pre $E/lab-a/pre-activation.json $E/lab-b/pre-activation.json $E/lab-c/pre-activation.json \
    --active-baseline $E/lab-a/active-baseline.json $E/lab-b/active-baseline.json $E/lab-c/active-baseline.json \
    --converged $E/lab-a/converged.json $E/lab-b/converged.json $E/lab-c/converged.json \
    --cleanup $E/lab-a/post-cleanup.json $E/lab-b/post-cleanup.json $E/lab-c/post-cleanup.json > "$E/comparison.json"; rc=$?
  jq -c '{status,canonical_convergence_evidenced,ha_claim,failure_count:(.failures|length),new:.new_incomplete_attempts,accounted:.accounted_incomplete_attempts,unaccounted:.unaccounted_incomplete_attempts,preexisting:.preexisting_incomplete_attempts,terminal:.terminal_attempts}' "$E/comparison.json" 2>/dev/null || head -c 400 "$E/comparison.json"
  echo "comparator exit=$rc"; exit $rc;;
derive)  # the inspection derived from each preserved store must reproduce the live post-cleanup projection
  python3 - "$E" "$CANDIDATE" <<'PY'
import json, sys, hashlib, pathlib
E = pathlib.Path(sys.argv[1]); candidate = sys.argv[2]
fields = ['logical_history_sha256', 'audit_set_sha256', 'receipt_set_sha256', 'history_count', 'receipt_count',
          'audit_event_count', 'incomplete_attempt_count', 'sqlite_integrity_result']
out = {'schema': 'manager2-campaign-derivation/v1', 'hosts': {}, 'fields_compared': fields,
       'method': 'podmesh-managerd --inspect-store run on the host against a copy of the store preserved after the typed shutdown; the digests and counts it reports must equal those the live post-cleanup capture projected',
       'candidate_binary_sha256_on_workstation': hashlib.sha256(open(candidate, 'rb').read()).hexdigest()}
ok_all = True
for a in ('lab-a', 'lab-b', 'lab-c'):
    d = json.load(open(E / a / 'derived-inspection.json'))
    live = json.load(open(E / a / 'post-cleanup.json'))['inspection']
    derived = {f: d.get(f) for f in fields}
    derived['incomplete_attempt_count'] = len(d.get('incomplete_attempts', []))
    mismatches = [f for f in fields if derived.get(f) != live.get(f)]
    ok_all &= not mismatches
    out['hosts'][a] = {'reproduced': not mismatches, 'mismatches': mismatches,
                       'store_sha256': open(E / a / 'preserved-store' / 'SHA256SUMS').read().split()[0],
                       'logical_history_sha256': live.get('logical_history_sha256'), 'audit_event_count': live.get('audit_event_count')}
out['derivation'] = 'reproduced on all three hosts' if ok_all else 'NOT reproduced on every host'
json.dump(out, open(E / 'derivation.json', 'w'), indent=2, sort_keys=True)
print(json.dumps({a: h['reproduced'] for a, h in out['hosts'].items()}), out['derivation'])
sys.exit(0 if ok_all else 5)
PY
  ;;
summary)  # public summary: aliases, hashes, counts, verdicts; never an address or a key
  python3 - "$W" <<'PY'
import json, sys, pathlib, re, datetime, hashlib
W = pathlib.Path(sys.argv[1]); E = W / 'live-evidence'; plan = json.load(open(W / 'campaign-plan.json'))
steps = json.load(open(W / 'steps.json')) if (W / 'steps.json').exists() else []
comparison = json.load(open(E / 'comparison.json')) if (E / 'comparison.json').exists() else None
derivation = json.load(open(E / 'derivation.json')) if (E / 'derivation.json').exists() else None
harness = {}
for line in open(W / 'campaign.log'):
    m = re.match(r'harness ([0-9a-f]{64}) (\S+)', line)
    if m: harness[m.group(2)] = m.group(1)
summary = {
  'schema_version': 'podmesh-manager-live-activation-public-summary/v2',
  'campaign': plan['campaign'], 'date_utc': datetime.datetime.now(datetime.timezone.utc).date().isoformat(),
  # The verdict is the comparator's and the derivation's, over live phases that all succeeded; a driver
  # phase that failed and was repaired is listed as an incident, never hidden and never counted as a
  # failure of the candidate. A live phase that failed makes the campaign FAIL whatever else happened.
  'result': ('PASS' if comparison and comparison.get('status') == 'PASS' and derivation and derivation.get('derivation', '').startswith('reproduced')
             and all(s['exit'] == 0 for s in steps if s['phase'].split()[0] in ('prepare-dirs', 'activate', 'observe', 'wait-converged', 'converged', 'rollback')) else 'FAIL'),
  'driver_incidents': [s for s in steps if s['exit'] != 0],
  'does_not_qualify': plan['does_not_qualify'],
  'steps': steps, 'harness_sha256': harness,
  'comparator': None if not comparison else dict({k: comparison.get(k) for k in ('status', 'canonical_convergence_evidenced', 'ha_claim')},
      failures=len(comparison.get('failures', [])), undecided_conditions=len(comparison.get('undecided_conditions', [])),
      unmatched_inbound_rows=len(comparison.get('unmatched_inbound_rows', [])),
      **{k: v for k, v in (comparison.get('incomplete_attempt_accounting') or {}).items() if isinstance(v, (int, str))}),
  'derivation': derivation,
  'preserved': {'raw_stores': [f'live-evidence/{a}/preserved-store/manager.sqlite' for a in plan['host_order']],
                'inspection_output': [f'live-evidence/{a}/derived-inspection.json (private: raw identifiers)' for a in plan['host_order']],
                'comparator_input': 'the twelve stage files and their sidecars', 'comparator_result': 'live-evidence/comparison.json'},
}
json.dump(summary, open(W / 'campaign-summary.json', 'w'), indent=2, sort_keys=True)
print(json.dumps({'result': summary['result'], 'steps': [(s['phase'], s['exit']) for s in steps]}))
PY
  ;;
campaign)  # everything, in order, stopping at the first failure and rolling back what was activated
  : > "$W/steps.json"; echo '[]' > "$W/steps.json"; activated=()
  step() { local name=$1; shift; local rc=0; "$0" "$name" "$@" || rc=$?
    jq --arg p "$name $*" --argjson rc "$rc" --arg t "$(date -u +%FT%TZ)" '. + [{"phase":$p,"exit":$rc,"at":$t}]' "$W/steps.json" > "$W/steps.json.tmp" && mv "$W/steps.json.tmp" "$W/steps.json"
    return $rc; }
  fail() { echo "CAMPAIGN FAILED at: $1" >&2; for a in "${activated[@]:-}"; do [ -n "$a" ] && step rollback "$a" || true; done; "$0" summary || true; exit 1; }
  step prepare-dirs || fail prepare-dirs
  for a in lab-a lab-b lab-c; do step activate "$a" || fail "activate $a"; activated+=("$a"); done
  for a in lab-a lab-b lab-c; do step observe "$a" || fail "observe $a"; done
  step wait-converged || fail wait-converged
  for a in lab-a lab-b lab-c; do step converged "$a" || fail "converged $a"; done
  for a in lab-a lab-b lab-c; do step rollback "$a" || fail "rollback $a"; done
  for a in lab-a lab-b lab-c; do step preserve "$a" || fail "preserve $a"; done
  step fetch || fail fetch
  step compare || true          # a FAIL comparison is a result, not a broken campaign: the summary carries it
  step derive || true
  "$0" summary;;
finish)  # resume after the live phases: preserve, fetch, compare, derive, summary, each recorded
  step() { local name=$1; shift; local rc=0; "$0" "$name" "$@" || rc=$?
    jq --arg p "$name $*" --argjson rc "$rc" --arg t "$(date -u +%FT%TZ)" '. + [{"phase":$p,"exit":$rc,"at":$t}]' "$W/steps.json" > "$W/steps.json.tmp" && mv "$W/steps.json.tmp" "$W/steps.json"
    return $rc; }
  for a in lab-a lab-b lab-c; do step preserve "$a" || { echo "preserve $a failed" >&2; "$0" summary; exit 1; }; done
  step fetch || { "$0" summary; exit 1; }
  step compare || true
  step derive || true
  "$0" summary;;
*) echo "usage: $0 campaign | finish | prepare-dirs | activate <alias> | observe <alias> | inspect | wait-converged | converged <alias> | rollback <alias> | preserve <alias> | fetch | compare | derive | summary" >&2; exit 2;;
esac
