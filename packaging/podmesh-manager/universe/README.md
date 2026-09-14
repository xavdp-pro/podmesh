# The manager as a PodMesh universe (candidate M-U1)

Codex's decision of 2026-09-14: the manager is one logical universe; PodMesh's activation,
recovery points and epoch screen are its only exclusive-role enforcement. This directory is
the smallest universe definition of the packaged resident, and what it taught.

Build on the active host, privately, with a configuration derived from `config.template.json`
(the UUIDs are identifiers of the laboratory universe and stay out of Git; there are no keys):

    podman build --network=host -t localhost/podmesh-manager-universe:m-u1 .

then `create` the universe from the image ID with the command `/usr/local/bin/manager-universe`
and drive it with `tools/ha-standby.py` from the main tree. `tests/check-manager-universe-ha.py`
there runs the HA-10 shape on three hosts and proves, by the frozen candidate's own
`--inspect-store` on the exported stores, that the manager's durable state follows the universe.

## What the universe contract imposes, and what the entrypoint does about it

- **No network.** The resident runs in `authenticated-static-peers` mode bound to loopback with
  no peer: it serves its control socket and exchanges nothing. Replication between replicas
  cannot be exercised inside a universe until universes have a network — a PodMesh contract
  decision, not a manager one.
- **No exec, no mounts.** The control socket is unreachable from the host. The entrypoint is the
  only process that can reach it, and it uses that for two things: it turns the container's stop
  signal into the resident's typed `shutdown` (the resident handles no signal itself, and a PID 1
  that ignores SIGTERM makes every PodMesh stop escalate, which refuses every capture), and it
  records each start as a `boot` fact in the replica's own scope, so the store has content to
  move. Both are logged to the container's output, the only channel out.
- **Overlay copy-up.** A store that arrives in an image layer (a restored recovery point) is
  copied up on its first write, which changes its device and inode between the resident's
  read-only preflight and its open; the resident refuses that as a swapped store
  (`manager store path changed after read-only preflight`). The entrypoint rewrites the store
  into the writable layer once before the resident starts. A stale control socket file from the
  previous incarnation is removed for the same reason.

## Measured on three lab hosts (2026-09-14)

Two boot facts on the active host; the point carried to two standbys; after the takeover the
promoted universe on the standby held three boot facts (the active host's last capture) with
integrity ok and the same digests as the active host's store; the resident started there and
chained a fourth; the other standby refused a stale epoch-1 permit; the old active was refused.
Nothing here proves a partition, a host loss, DNS, remote transport, replication, or that an
agent can operate the manager inside the universe.
