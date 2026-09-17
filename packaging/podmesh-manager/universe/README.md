# The manager as a PodMesh universe (candidate M-U1)

Two root images implement the same universe contract and pass the same proof: **`Containerfile.alpine`
is the default**, per the image policy of 2026-09-14, with a musl build of the resident
(`MUSL-BUILD.md`); `Containerfile` (Debian 13) is the documented compatibility branch that carries
the frozen glibc candidate as it is (`DEBIAN-EXCEPTION.md`, `ALPINE-PROOF.md`).

Codex's decision of 2026-09-14: the manager is one logical universe; PodMesh's activation,
recovery points and epoch screen are its only exclusive-role enforcement. This directory is
the smallest universe definition of the packaged resident, and what it taught.

**This is a single-replica portability fixture, not the configuration of the replicated
manager.** `config.single-replica-fixture.template.json` declares one replica, no peer and one
scope, because a universe has no network: a three-replica topology here would only manufacture
failed exchange attempts. The replicated manager's configuration is the campaign's, and it waits
for the universe network. Build on the active host, privately, with a configuration derived
from the template (the UUIDs are identifiers of the laboratory universe and stay out of Git;
there are no keys):

    podman build --network=host -t localhost/podmesh-manager-universe:m-u1-alpine -f Containerfile.alpine .   # default
    podman build --network=host -t localhost/podmesh-manager-universe:m-u1 .                                  # Debian branch

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
  move. Both are logged to the container's output, the only channel out. **Both are terminal:**
  a boot fact that is not observed ends the universe with exit 2 before PodMesh's observation
  window closes, so the start is "not running when observed"; a typed shutdown that is not
  acknowledged ends it with exit 3 within the stop timeout, so PodMesh reports an honest failed
  stop with no escalation, and the capture cycle refuses to take a point after it. The check
  injects both (`--fault boot`, `--fault shutdown`). One bounded exception, added on 2026-09-15
  after a replica restarted right after its typed stop answered `append_observation_uncertain`
  once and was refused a start it could have had: an `uncertain` or `busy` answer is retried
  with the **same** operation ID, half a second apart, within the start's 25-second budget — the
  resident replays an operation ID it has already appended rather than appending it twice, so
  the fact is observed once or the start fails as before. Since 2026-09-17 a
  `catching_up` answer is retried the same way: the resident appends nothing before it has
  caught up with its peers, so that a store which lost some of this replica's own facts gets
  them back before the boot fact takes the next sequence number. A store with no fact of its
  own and a peer that stays unreachable is therefore refused a start at the budget. Any other
  answer stays terminal, and the injected boot fault (a socket that does not exist) still exits
  2 at once.
- **Attested binary.** The check exports `/usr/lib/podmesh-manager/podmesh-managerd` from the
  universe and refuses any inspection unless its SHA-256 equals the inspector's on the
  workstation; both digests are recorded in the result.
- **Alpine first, Debian as a recorded exception.** The image policy of 2026-09-14
  (`/tmp/podmesh-claude/IMAGE-POLICY-UNIVERSES-2026-09-14.md`, kept beside the canon by the
  operator) makes Alpine the default root image; `DEBIAN-EXCEPTION.md` records this universe's
  exception in the policy's form — component, Alpine limitation, Debian dependency, smoke test —
  and why it is the root and how it goes away. See `ALPINE-PROOF.md`: the frozen candidate is a glibc ≥ 2.34 executable
  that cannot be loaded on musl, with or without `gcompat` (`fcntl64: symbol not found`). The
  Debian 13 base is the recorded exception; an Alpine image needs a musl build of the candidate.
- **Overlay copy-up.** A store that arrives in an image layer (a restored recovery point) is
  copied up on its first write, which changes its device and inode between the resident's
  read-only preflight and its open; the resident refuses that as a swapped store
  (`manager store path changed after read-only preflight`). The entrypoint rewrites the store
  into the writable layer once before the resident starts. A stale control socket file from the
  previous incarnation is removed for the same reason.

## Measured on three lab hosts (2026-09-14, rerun after Codex's review; then again on the Alpine image)

The Alpine root image (59.7 MB against 120 MB) passed the identical proof with the musl binary
attested equal to the inspector (`4111e487…`): injected failures, typed stop, store 2 → 3 → 4
chained facts through capture, restore, promotion and restart, stale permit refused.

Binary in the universe attested equal to the inspector (`cbd5020a…`); injected boot-fact failure
→ exit 2, not running when observed; injected typed-shutdown failure → exit 3, stop reported
failed without escalation, capture refused. Then the honest run: two boot facts on the active host; the point carried to two standbys; after the takeover the
promoted universe on the standby held three boot facts (the active host's last capture) with
integrity ok and the same digests as the active host's store; the resident started there and
chained a fourth; the other standby refused a stale epoch-1 permit; the old active was refused.
Nothing here proves a partition, a host loss, DNS, remote transport, replication, or that an
agent can operate the manager inside the universe.

## The replicated set (M-U2)

`replicated/generate-replica-set.py` writes the three configurations of one logical manager
replicated as three universes on the managed network: one logical manager UUID, three replica
UUIDs bound to the three PodMesh host UUIDs, three owned scopes `m-u2/<alias>/observations`,
one distinct pair key per pair, and explicit authenticated endpoints at each replica's managed
address — no name is ever resolved. Its output splits public topology (`replica-set.json`) from
private material (each `<alias>/config.json`, with the pair keys), which stays out of Git and
**out of every image**: since 2026-09-15 (Codex's finding B2) the Alpine image is generic — one
image, no configuration, no key, built once per host as
`localhost/podmesh-manager-universe:m-u2-generic` — and a replica's configuration reaches its
host as a PodMesh secret (`secret_declare` from a root-only inbox copy) mounted into the universe
at `/etc/podmesh-manager/config.json`, root-only, at creation. Per-host images with a baked
configuration were the first M-U2 build and are no longer used by any suite. The entrypoint reads
the scope its replica owns from the configuration before appending the boot fact.
`tests/check-secrets-image-free.py` (main tree) scans the image save and the container export for
every pair key and identity.

Measured on 2026-09-14 (`tests/check-manager-replicas-managed.py`, main tree): three replicas
running concurrently across three hosts converged their facts — three boot facts, byte-identical
sets, authenticated imports from both peers on each replica, exchange audit rows — verified from
outside with all three running. What it does not prove: the governor role, takeover, partitions,
and agent access to the control API.
