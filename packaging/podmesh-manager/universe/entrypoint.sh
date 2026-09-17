#!/bin/sh
# PID 1 of the manager universe. It runs the resident, records each start as a fact through the
# control socket only this process can reach, and turns the container's stop signal into the
# resident's typed shutdown. Every failure is terminal and visible from outside: a start whose boot
# fact is not observed exits 2 (the universe is then "not running when observed"), a stop whose typed
# shutdown is not acknowledged exits 3 (an honest failed stop, never a silent wait for escalation).
# The optional argument `--fault boot|shutdown` injects those failures for the laboratory check.
#
# The start has a budget. Whoever starts the universe observes it for at most 30 seconds (PodMesh's
# bound on observe_seconds), so a boot that cannot be observed ends, with exit 2, within 25 seconds of
# this script's start: a start is never recorded as running for a replica that is about to fail.
set -eu
umask 077
budget_seconds=25
started_at=$(date +%s)
elapsed() { echo $(( $(date +%s) - started_at )); }
fault=${2:-none}
[ "${1:-}" = "--fault" ] || fault=none
mkdir -p /run/podmesh-manager /var/lib/podmesh-manager && chmod 700 /run/podmesh-manager /var/lib/podmesh-manager
# A universe restored from a capture may carry the previous incarnation's control socket file; the
# resident refuses an existing path rather than unlink it, so the stale file is removed here, where
# it is known to belong to no running process (this is PID 1, and nothing else has started).
rm -f /run/podmesh-manager/control.sock
# A store that arrived in an image layer (a restored recovery point) is copied up by the overlay
# filesystem on its first write, which changes its inode between the resident's read-only preflight
# and its open; the resident refuses that as a swapped store. Rewriting the file here, before the
# resident starts, puts it in the writable layer once, with a stable identity.
for f in /var/lib/podmesh-manager/manager.sqlite /var/lib/podmesh-manager/manager.sqlite-wal /var/lib/podmesh-manager/manager.sqlite-shm; do
  if [ -f "$f" ]; then cp -p -- "$f" "$f.copyup" && mv -f -- "$f.copyup" "$f"; fi
done
export PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers
/usr/lib/podmesh-manager/podmesh-managerd --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager --runtime-dir /run/podmesh-manager &
child=$!
echo "manager-universe: resident started pid=$child"

# The origin a publishing connector proxies to: a small HTTP responder on port 8080, fail-closed
# on the active manager's mark (a root-only file PodMesh writes at the exclusive publication, under
# the epoch gate, and removes at the withdrawal or the fence). Without it every path answers 503: a
# connector that reaches a replica which is not the active manager gets nothing. It decides nothing
# about the role. It also carries the administration app (/admin, React over an Express API, the
# same stack as the operator's other tools): an administrator is a replicated fact, and the FIRST
# one is never created there -- it is written from the host, as root, through PodMesh's control
# door. See origin/server/app.mjs.
node /usr/lib/podmesh-manager/origin/server/index.mjs &
origin=$!
echo "manager-universe: origin responder started pid=$origin (fail-closed until PodMesh marks this replica as the active manager)"

control() { # control <socket> <json>: one typed request, read to end of stream, reply on stdout
  python3 - "$1" "$2" <<'EOF'
import socket, sys, os, time
path, req = sys.argv[1], sys.argv[2].encode()
for _ in range(20):
    if os.path.exists(path):
        break
    time.sleep(0.1)
s = socket.socket(socket.AF_UNIX); s.settimeout(5); s.connect(path); s.sendall(req); s.shutdown(socket.SHUT_WR)
reply = b""
while True:
    chunk = s.recv(4096)
    if not chunk:
        break
    reply += chunk
print(reply.decode().strip())
EOF
}
socket_path=/run/podmesh-manager/control.sock
[ "$fault" = boot ] && boot_socket=/run/podmesh-manager/no-such.sock || boot_socket=$socket_path
[ "$fault" = shutdown ] && stop_socket=/run/podmesh-manager/no-such.sock || stop_socket=$socket_path

# The resident binds its control socket only after it has opened its store and verified it whole,
# which takes longer as the store grows (about a second for thirty thousand audit rows, measured on
# 2026-09-17). Wait for the socket within the budget, and stop waiting as soon as the resident is gone.
while [ ! -S "$socket_path" ]; do
  if ! kill -0 "$child" 2>/dev/null; then
    echo "manager-universe: RESIDENT EXITED before binding its control socket; refusing to run"
    exit 2
  fi
  if [ "$(elapsed)" -ge "$budget_seconds" ]; then
    echo "manager-universe: CONTROL SOCKET NOT BOUND within ${budget_seconds}s; refusing to run"
    kill -KILL "$child" 2>/dev/null || true
    exit 2
  fi
  sleep 0.2
done
echo "manager-universe: control socket bound after $(elapsed)s"

# Readiness requirement: the start is not a start until the resident has observed this boot's fact.
boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || echo unknown)
opid=$(cat /proc/sys/kernel/random/uuid)
# The scope this replica owns, read from its own configuration: a fact appended outside an owned
# scope is refused by the resident, and rightly so.
scope=$(python3 -c 'import json,sys; c=json.load(open("/etc/podmesh-manager/config.json")); r=c["network"]["replica_id"]; print([g["scope"] for g in c["network"]["manager"]["grants"] if g["owner_replica_id"]==r][0])')
# The same operation ID is retried, bounded, when the resident answers `uncertain` or `busy`: an
# uncertain append is one whose outcome the resident could not tell (a store still opening after
# the previous incarnation's shutdown, a worker past its deadline), and the resident replays an
# operation ID it has already appended rather than appending it twice, so the retry is safe and
# the fact ends up observed once. Any other answer is terminal. Measured on 2026-09-15: a replica
# restarted right after its typed stop answered `append_observation_uncertain` once, and this
# entrypoint refused to run -- correctly, on that contract, and needlessly. The retries stop at the
# start's budget, not at a count.
#
# `catching_up` is retried the same way. The resident appends nothing before it has caught up with
# its peers -- an import from each, or a receipt showing that the peer holds nothing it lacks -- so
# that a store which lost some of this replica's own facts gets them back before the boot fact
# takes the next sequence number, instead of reusing one its peers hold with other bytes, which
# they would refuse for good. It touched nothing, so the retry is safe. With every peer up this
# takes a few exchanges. A store whose latest fact of its own it appended itself also appends once
# its catch-up window has elapsed (15 s at most, from the moment its control socket is bound), it
# has reached one peer and tried every other: this clock counts whole seconds, so the last attempt
# is certain only 24 s after this script started, and attempts are about 0.6 s apart, so such a
# start fits the budget while the socket is bound within about 8 s. An emptied store, or one that
# only imported its own facts back, is refused a start at the budget while a peer stays
# unreachable, visibly, rather than forking its history; so is a replica that reaches no peer.
observed=no; attempt=0
while :; do
  attempt=$((attempt + 1))
  reply=$(control "$boot_socket" "{\"operation\":\"append_observation\",\"operation_id\":\"$opid\",\"scope\":\"$scope\",\"subject\":\"boot\",\"value\":\"boot-$boot\"}" 2>&1) || true
  if printf '%s' "$reply" | grep -q '"result":"observed"'; then observed=yes; break; fi
  if printf '%s' "$reply" | grep -q 'append_observation_uncertain\|append_observation_busy\|append_observation_catching_up' && [ "$(elapsed)" -lt "$budget_seconds" ]; then
    echo "manager-universe: boot fact attempt $attempt: $(printf '%s' "$reply" | tail -1 | cut -c1-80); retrying the same operation"
    sleep 0.5; continue
  fi
  break
done
if [ "$observed" = yes ]; then
  echo "manager-universe: boot fact observed (attempt $attempt, $(elapsed)s after the start): $(printf '%s' "$reply" | cut -c1-120)"
else
  echo "manager-universe: BOOT FACT NOT OBSERVED after $attempt attempt(s) and $(elapsed)s; refusing to run: $(printf '%s' "$reply" | tail -1 | cut -c1-200)"
  kill -KILL "$child" 2>/dev/null || true
  exit 2
fi

shutdown_rc=0
shutdown() {
  echo "manager-universe: stop signal received; requesting the typed shutdown"
  if reply=$(control "$stop_socket" '{"operation":"shutdown"}' 2>&1) && printf '%s' "$reply" | grep -q '"shutdown_requested":true'; then
    echo "manager-universe: shutdown acknowledged: $reply"
  else
    echo "manager-universe: TYPED SHUTDOWN FAILED: $(printf '%s' "$reply" | tail -1 | cut -c1-200); stopping the resident by force and exiting 3"
    shutdown_rc=3
    kill -KILL "$child" 2>/dev/null || true
  fi
}
trap shutdown TERM INT
rc=0
while :; do
  set +e; wait "$child"; rc=$?; set -e
  kill -0 "$child" 2>/dev/null || break
done
kill "$origin" 2>/dev/null || true
if [ "$shutdown_rc" -ne 0 ]; then echo "manager-universe: resident ended after a failed typed shutdown"; exit "$shutdown_rc"; fi
echo "manager-universe: resident exited rc=$rc"
exit "$rc"
