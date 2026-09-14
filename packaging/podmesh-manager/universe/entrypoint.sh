#!/bin/sh
# PID 1 of the manager universe: runs the resident and turns the container's stop signal into the
# resident's own typed shutdown, so that a PodMesh stop is graceful and a capture keeps its class.
# The acknowledgement is written to the container log, which is the only channel out of a universe.
set -u
umask 077
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
  [ -f "$f" ] && cp -p -- "$f" "$f.copyup" && mv -f -- "$f.copyup" "$f"
done
export PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers
/usr/lib/podmesh-manager/podmesh-managerd --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager --runtime-dir /run/podmesh-manager &
child=$!
echo "manager-universe: resident started pid=$child"
# Each start records itself as a fact in the replica's own scope, through the control socket only
# this process can reach: the store then carries content that a takeover has to move, not merely a
# schema. The boot identity is the value; the operation ID is fresh, so a replayed start is not.
boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || echo unknown)
opid=$(cat /proc/sys/kernel/random/uuid)
python3 - "$boot" "$opid" <<'EOF' 2>&1 | sed 's/^/manager-universe: boot fact: /'
import socket, json, sys, time, os
path = '/run/podmesh-manager/control.sock'
for _ in range(100):
    if os.path.exists(path): break
    time.sleep(0.1)
req = json.dumps({"operation": "append_observation", "operation_id": sys.argv[2], "scope": "lab/manager-universe/observations",
                  "subject": "boot", "value": "boot-" + sys.argv[1]}).encode()
s = socket.socket(socket.AF_UNIX); s.settimeout(15); s.connect(path); s.sendall(req); s.shutdown(socket.SHUT_WR)
reply = b""
while True:
    chunk = s.recv(4096)
    if not chunk: break
    reply += chunk
print(reply.decode().strip()[:300])
EOF
shutdown() {
  echo "manager-universe: stop signal received; requesting the typed shutdown"
  # The resident reads the request to end of stream: no newline, then the write side is closed.
  python3 - <<'EOF' 2>&1 | sed 's/^/manager-universe: shutdown reply: /'
import socket
s = socket.socket(socket.AF_UNIX); s.settimeout(10); s.connect('/run/podmesh-manager/control.sock')
s.sendall(b'{"operation":"shutdown"}'); s.shutdown(socket.SHUT_WR)
reply = b""
while True:
    chunk = s.recv(4096)
    if not chunk:
        break
    reply += chunk
print(reply.decode().strip())
EOF
}
trap shutdown TERM INT
rc=0
while :; do
  wait $child; rc=$?
  kill -0 $child 2>/dev/null || break
done
echo "manager-universe: resident exited rc=$rc"
exit $rc
