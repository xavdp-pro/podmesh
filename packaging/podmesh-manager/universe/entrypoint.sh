#!/bin/sh
# PID 1 of the manager universe. It runs the resident, records each start as a fact through the
# control socket only this process can reach, and turns the container's stop signal into the
# resident's typed shutdown.
#
# Readiness contract. RUNNING means the resident is up and exchanging with its peers. READY means
# this boot's fact is observed. A replica that has not caught up with its peers is running and not
# yet ready: it keeps exchanging, so that the replicas started after it can catch up with it, and
# this script keeps asking for the boot fact with no deadline, one line of the resident's catch-up
# state in the log every 30 seconds. Readiness is visible in this log ("boot fact observed") and in
# the resident's status (`catch_up.appends_observed`, `catch_up.caught_up`). Whoever waits for a
# replica to be ready waits for that line, not for the container to be up.
#
# One catch-up never ends and is therefore terminal too: a replica whose history already forked from
# a peer's is refused every import from it and refused by it, so it is reached and never caught up
# with. The resident names that in its status, and this start exits 2 with the event ID and the peer
# in the log rather than running for ever without being ready.
#
# Every other failure is terminal and visible from outside: a boot fact refused for any other reason
# exits 2 (the universe is then "not running when observed"), a stop whose typed shutdown is not
# acknowledged exits 3 (an honest failed stop, never a silent wait for escalation). The optional
# argument `--fault boot|shutdown` injects those failures for the laboratory check.
#
# The start has a budget of 25 seconds. It bounds the control socket's bind and the retries of an
# `uncertain` or `busy` answer: whoever starts the universe observes it for at most 30 seconds
# (PodMesh's bound on observe_seconds), so those failures end within 25 seconds of this script's
# start and a start is never recorded as running for a replica that is about to fail. It does not
# bound catching up, which is not a failure and can last as long as a peer stays out of reach; the
# budget of the `uncertain` and `busy` retries restarts after each `catching_up` answer.
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
# The active manager's mark is PodMesh's to write, at publisher_start under the epoch gate, and this
# replica's /run is its overlay, not a tmpfs: a mark written before a stop is still here after the
# start (observed on lab-a, 2026-09-18), and a withdrawal made while the replica was stopped found no
# running universe to remove it from. Removed here at every start, at its path and at the previous
# one, before the origin starts: the origin answers 503 until PodMesh writes the mark again, so this
# replica never claims a role it may no longer hold.
rm -f /run/podmesh-manager/active-manager.json /run/podmesh-manager/governor.json
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

# From here the typed shutdown is possible, so the stop signal is taken from here: a replica that is
# still catching up must stop as honestly as a ready one, and it may wait a long time.
shutdown_rc=0
stopping=no
shutdown() {
  stopping=yes
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

# The resident's catch-up state, in two lines: what it holds of its peers, then what blocks its
# readiness (`reason=waiting_for_peers`, `reason=refused_by_peer`, `reason=identity_collision`, or
# `reason=none` once it is caught up). The first line goes to the log; the second decides whether
# waiting can end in readiness at all.
catch_up_state() {
  control "$socket_path" '{"operation":"status"}' 2>/dev/null | python3 -c '
import json, sys
try:
    c = json.loads(sys.stdin.read().strip().splitlines()[-1])["catch_up"]
except Exception:
    print("catch-up state unavailable")
    print("reason=unknown event_id=- peers=-")
    sys.exit(0)
def names(key):
    return ",".join(c.get(key) or []) or "-"
print(
    "imported=%s matched=%s missing=%s ahead=%s not_attempted=%s own_facts_at_start=%s"
    " latest_own_fact_appended_locally=%s window_ms=%s"
    % (names("peers_imported"), names("peers_matched"), names("peers_missing"), names("peers_ahead"),
       names("peers_not_attempted"), c.get("own_facts_at_start"),
       c.get("latest_own_fact_appended_locally"), c.get("window_ms"))
)
blocked = c.get("blocked_by") or {}
print(
    "reason=%s event_id=%s peers=%s"
    % (blocked.get("reason", "none"), blocked.get("event_id") or "-",
       ",".join(blocked.get("peers") or []) or "-")
)
' 2>/dev/null || printf '%s\n%s\n' "catch-up state unavailable" "reason=unknown event_id=- peers=-"
}

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
# the fact ends up observed once. Measured on 2026-09-15: a replica restarted right after its typed
# stop answered `append_observation_uncertain` once, and this entrypoint refused to run --
# correctly, on that contract, and needlessly. Those retries stop at the budget, not at a count.
#
# `catching_up` is retried with no deadline. The resident appends nothing before it has caught up
# with its peers -- an import from each, or a receipt showing that the peer holds nothing it lacks
# -- so that a store which lost some of this replica's own facts gets them back before the boot
# fact takes the next sequence number, instead of reusing one its peers hold with other bytes,
# which they would refuse for good. It touched nothing, so the retry is safe. With every peer up
# this takes a few exchanges. A store whose latest fact of its own it appended itself also appends
# once its catch-up window has elapsed (15 s by default, counted from the moment the resident
# began exchanging, that is after its store's first complete verification) and a fresh round of
# attempts, taken after that end, has caught it up with at least one peer and left every other one
# unreached. An emptied store, one that only imported its own facts back, and a replica that
# reaches no peer wait here for as long as it takes: they stay running and keep exchanging, which
# is what lets the other replicas catch up with them, and they are never ready until their boot
# fact is observed.
#
# One catch-up never ends: a history that already forked from a peer's. Every import from that peer
# is refused as an event identity collision and every push to it is refused too, so it is reached
# and never caught up with, and no window forgives a peer that answers. The resident says so in its
# status (`catch_up.blocked_by.reason` is `identity_collision`), and this start is then terminal:
# waiting would leave a container running for ever without ever being ready, where PodMesh must
# record a universe that is not running when observed. It needs an operator, not time.
#
# Any other answer is terminal.
observed=no; attempt=0; catching_up=0; reported_at=0; checked_at=0; retry_since=$started_at
while :; do
  attempt=$((attempt + 1))
  reply=$(control "$boot_socket" "{\"operation\":\"append_observation\",\"operation_id\":\"$opid\",\"scope\":\"$scope\",\"subject\":\"boot\",\"value\":\"boot-$boot\"}" 2>&1) || true
  if printf '%s' "$reply" | grep -q '"result":"observed"'; then observed=yes; break; fi
  [ "$stopping" = yes ] && break
  if printf '%s' "$reply" | grep -q 'append_observation_catching_up'; then
    now=$(date +%s)
    retry_since=$now
    # The state is read every five seconds -- a collision is only known once an exchange has carried
    # it -- and one line is logged every thirty.
    if [ "$catching_up" -eq 0 ] || [ $(( now - checked_at )) -ge 5 ]; then
      checked_at=$now
      state=$(catch_up_state)
      summary=$(printf '%s\n' "$state" | sed -n 1p)
      blocked=$(printf '%s\n' "$state" | sed -n 2p)
      case "$blocked" in
        reason=identity_collision*)
          echo "manager-universe: CATCH-UP BLOCKED BY AN EVENT IDENTITY COLLISION ($blocked): this replica's history forked from its peer's, no import can carry the facts it lacks, and no waiting resolves it; refusing to run"
          kill -KILL "$child" 2>/dev/null || true
          exit 2
          ;;
      esac
      if [ "$catching_up" -eq 0 ] || [ $(( now - reported_at )) -ge 30 ]; then
        reported_at=$now
        echo "manager-universe: running, not ready: the resident is catching up with its peers ($(elapsed)s after the start, attempt $attempt); $summary; $blocked"
      fi
    fi
    catching_up=$((catching_up + 1))
    # Poll closely at first, so that a catch-up of a few exchanges is not delayed, then slowly: a
    # replica can stay here for hours while a peer is out of reach.
    if [ "$(elapsed)" -lt 60 ]; then sleep 0.5; else sleep 2; fi
    continue
  fi
  if printf '%s' "$reply" | grep -q 'append_observation_uncertain\|append_observation_busy' && [ $(( $(date +%s) - retry_since )) -lt "$budget_seconds" ]; then
    echo "manager-universe: boot fact attempt $attempt: $(printf '%s' "$reply" | tail -1 | cut -c1-80); retrying the same operation"
    sleep 0.5; continue
  fi
  break
done
if [ "$observed" = yes ]; then
  echo "manager-universe: boot fact observed (attempt $attempt, $(elapsed)s after the start): $(printf '%s' "$reply" | cut -c1-120)"
elif [ "$stopping" = yes ]; then
  echo "manager-universe: stopping before this boot's fact was observed, after $attempt attempt(s) and $(elapsed)s"
else
  echo "manager-universe: BOOT FACT NOT OBSERVED after $attempt attempt(s) and $(elapsed)s; refusing to run: $(printf '%s' "$reply" | tail -1 | cut -c1-200)"
  kill -KILL "$child" 2>/dev/null || true
  exit 2
fi

rc=0
while :; do
  set +e; wait "$child"; rc=$?; set -e
  kill -0 "$child" 2>/dev/null || break
done
kill "$origin" 2>/dev/null || true
if [ "$shutdown_rc" -ne 0 ]; then echo "manager-universe: resident ended after a failed typed shutdown"; exit "$shutdown_rc"; fi
echo "manager-universe: resident exited rc=$rc"
exit "$rc"
