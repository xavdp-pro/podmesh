#!/bin/sh
# PID 1 of the manager universe. It runs the resident, records each start as a fact through the
# control socket only this process can reach, and turns the container's stop signal into the
# resident's typed shutdown. Every failure is terminal and visible from outside: a start whose boot
# fact is not observed exits 2 (the universe is then "not running when observed"), a stop whose typed
# shutdown is not acknowledged exits 3 (an honest failed stop, never a silent wait for escalation).
# The optional argument `--fault boot|shutdown` injects those failures for the laboratory check.
set -eu
umask 077
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

control() { # control <socket> <json>: one typed request, read to end of stream, reply on stdout
  python3 - "$1" "$2" <<'EOF'
import socket, sys, os, time
path, req = sys.argv[1], sys.argv[2].encode()
for _ in range(50):
    if os.path.exists(path):
        break
    time.sleep(0.1)
s = socket.socket(socket.AF_UNIX); s.settimeout(15); s.connect(path); s.sendall(req); s.shutdown(socket.SHUT_WR)
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
# entrypoint refused to run -- correctly, on that contract, and needlessly.
observed=no; attempt=0
while [ $attempt -lt 10 ]; do
  attempt=$((attempt + 1))
  reply=$(control "$boot_socket" "{\"operation\":\"append_observation\",\"operation_id\":\"$opid\",\"scope\":\"$scope\",\"subject\":\"boot\",\"value\":\"boot-$boot\"}" 2>&1) || true
  if printf '%s' "$reply" | grep -q '"result":"observed"'; then observed=yes; break; fi
  if printf '%s' "$reply" | grep -q 'append_observation_uncertain\|append_observation_busy'; then
    echo "manager-universe: boot fact attempt $attempt: $(printf '%s' "$reply" | tail -1 | cut -c1-80); retrying the same operation"
    sleep 1; continue
  fi
  break
done
if [ "$observed" = yes ]; then
  echo "manager-universe: boot fact observed (attempt $attempt): $(printf '%s' "$reply" | cut -c1-120)"
else
  echo "manager-universe: BOOT FACT NOT OBSERVED after $attempt attempt(s); refusing to run: $(printf '%s' "$reply" | tail -1 | cut -c1-200)"
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
if [ "$shutdown_rc" -ne 0 ]; then echo "manager-universe: resident ended after a failed typed shutdown"; exit "$shutdown_rc"; fi
echo "manager-universe: resident exited rc=$rc"
exit "$rc"
