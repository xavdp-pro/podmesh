# The manager as a PodMesh universe (candidate M-U1)

Two root images implement the same universe contract and pass the same proof: **`Containerfile.alpine`
is the default**, per the image policy of 2026-09-14, with a musl build of the resident
(`MUSL-BUILD.md`); `Containerfile` (Debian 13) is the documented compatibility branch that carries
the frozen glibc candidate as it is (`DEBIAN-EXCEPTION.md`, `ALPINE-PROOF.md`).

For a musl build from a Debian build container, install `musl-tools` and the Rust
`x86_64-unknown-linux-musl` target, then build the resident with an explicit static link:

```sh
RUSTFLAGS='-C relocation-model=static -C link-arg=-static' \
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl
```

Check that `file` reports a statically linked executable, that `--version` exits successfully,
and that the resident passes its decision tests before putting it in an image. A build that
returns exit code zero but crashes on invocation is not an image candidate. Record the exact
source, toolchain, SHA-256 and image digest with the qualification evidence.

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
  a boot fact refused for any reason but the two below ends the universe with exit 2 before
  PodMesh's observation window closes, so the start is "not running when observed"; a typed
  shutdown that is not acknowledged ends it with exit 3 within the stop timeout, so PodMesh
  reports an honest failed stop with no escalation, and the capture cycle refuses to take a point
  after it. The check injects both (`--fault boot`, `--fault shutdown`). One bounded exception,
  added on 2026-09-15 after a replica restarted right after its typed stop answered
  `append_observation_uncertain` once and was refused a start it could have had: an `uncertain`
  or `busy` answer is retried with the **same** operation ID, half a second apart, within the
  start's 25-second budget — the resident replays an operation ID it has already appended rather
  than appending it twice, so the fact is observed once or the start fails as before.
- **Running is not ready** (the readiness contract, corrected 2026-09-17). **Running** means the
  resident is up and exchanging with its peers; **ready** means this boot's fact is observed. The
  resident appends nothing before it has caught up with its peers, so that a store which lost some
  of this replica's own facts gets them back before the boot fact takes the next sequence number;
  while it answers `catching_up` the entrypoint keeps asking, with the same operation ID and **no
  deadline**, and writes one line of the resident's catch-up state (peers imported, matched,
  missing, ahead) to the log every 30 seconds. The replica stays up and keeps exchanging, which is
  what lets the replicas started after it catch up with it — three replicas started 30 seconds
  apart all become ready, where the previous contract killed each of them at its budget. An emptied
  store, one that only imported its own facts back, and a replica that reaches no peer wait here
  for as long as it takes, visibly, rather than forking their history. Readiness is in the log
  (`boot fact observed`) and in the resident's status (`catch_up.appends_observed`,
  `catch_up.caught_up`); the roll tool waits for that line. The 25-second budget still bounds the
  socket bind and the `uncertain`/`busy` retries, and its clock for those restarts after each
  `catching_up` answer. The injected boot fault (a socket that does not exist) still exits 2 at
  once, and the stop signal is honoured from the moment the socket is bound, catching up or not.
- **One catch-up never ends, and that start is terminal.** A replica whose history already forked
  from a peer's is refused every import from that peer and refused by it, so the peer is reached and
  never caught up with, and no window forgives a peer that answers. The resident says so
  (`catch_up.blocked_by.reason` is `identity_collision`, with the peer and the event ID); the
  entrypoint reads that state every five seconds while it waits and, on that reason, logs the event
  ID and exits 2. PodMesh then records a universe that is not running when observed instead of a
  container that runs for ever without being ready. It needs an operator, not time.
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
  into the writable layer once before the resident starts.
- **A stale control socket file.** A universe restored from a capture can carry the previous
  incarnation's socket file, and the resident refuses a path that already exists rather than
  unlinking it (`UnixListener::bind`), so the entrypoint removes it — there, as PID 1 before
  anything else has started, it is known to belong to no running process. This has nothing to do
  with the copy-up above.

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
outside with all three running. What it does not prove: the active manager role, takeover, partitions,
and agent access to the control API.

## The replica's host state (V3-5)

A replica that votes keeps its signing key and its signing ledger outside the universe's state, and
checks the host's machine-id on a read-only mount (V3-4, the operator's custody decision and rule R4).
The universe gets them from PodMesh's `create` with `manager_host_state: <name>` (the node's
`LOCAL-API.md`), as three fixed bind mounts derived from the name. On a QEMU guest with a VM
generation device, the node also mounts its live witness read-only:

| In the universe | On the host | Mode |
| --- | --- | --- |
| `/run/podmesh-host/votes` | `<node state>/manager-host/<name>/votes` | read-write: `<key_id>.key`, `<key_id>.ledger` |
| `/run/podmesh-host/evidence` | `<node state>/manager-host/<name>/evidence` | read-only: the operator's readmission evidence |
| `/run/podmesh-host/machine-id` | `/etc/machine-id` | read-only |
| `/run/podmesh-host/vmgenid` | QEMU `etc/vmgenid_guid/raw`, when present | read-only |

The entrypoint sees `/run/podmesh-host/votes` and gives the resident `PODMESH_MANAGER_VOTE_DIR` and
`PODMESH_MANAGER_HOST_ID_FILE`; when present, it also exports
`PODMESH_MANAGER_GENERATION_ID_FILE`. The configuration names `votes.evidence_dir` =
`/run/podmesh-host/evidence`. Without the mounts, a configuration that votes does not start: the
resident refuses, and the start exits 2.
For a VM that may be restored from a snapshot, set `votes.require_generation_id` so a missing
external witness refuses every vote operation. The resident may still start to serve read-only
status and exchange facts, but it cannot sign until the witness is available and the ledger is
admitted. The witness detects a changed generation even
when RAM rollback preserves the old kernel boot ID. It does not prove an already in-flight
signature safe across a RAM rollback. Until that case is separately qualified, prohibit snapshots
that include VM RAM and their rollback while the VM can vote; use an isolated disk-only restore
followed by ledger readmission.

No recovery point, clone or migration carries the key or the ledger. PodMesh refuses mounts to live
captures, clones and migrations, and a stopped capture exports the root filesystem only. One universe
holds a name at a time: a second `create` with the same name is refused while the first holds it. A
`delete` releases the directory (renamed, never removed), and a roll that re-creates the replica under the
same name finds the key and the ledger once the old universe is gone.

The procedure, the seed never leaving its host:

1. On each host, as root: `replicated/vote-key.py --vote-dir <node state>/manager-host/<name>/votes
   --key-id <key_id>`. It writes the seed (0600) and prints the public half only.
2. Where the replica set was generated: `replicated/add-votes.py --dir <set> --key <alias>:<key_id>:<public
   key> ... --resource <uuid>:<lease>:<margin>:<renewal not_after> --require-generation-id`, with an optional `--baseline` for a
   resource moving from the gate. It adds each replica's `votes` section and its `votes/` and
   `proposals/` scopes, and prints the policy digest. The nodes' policies must name the same
   `authority_quorum`.
3. Declare each configuration as the replica's secret and create the universe with its
   `manager_host_state`.
4. Create each ledger (`manager_vote_ledger_init`, as the operator, through the node's bounded
   manager control door; do not enter the container). Gather the evidence into the host's evidence directory with the
   resident's `tools/collect-readmission-evidence.py`, and readmit (`manager_vote_ledger_readmit` with the
   printed digests, also through the door) once `retry_at` has passed.
5. On each host, enable the node's decision follow timer under its mandate (`DECISION-FOLLOW.md` of the
   node). Its `manager_universe` is the replica's universe.

`replicated/test_votes_tools.py` holds the two helpers. The node's tests hold the fixed mounts and
optional generation witness. Neither
has run on a laboratory host.
