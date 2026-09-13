# PodMesh Backup Server

Status: design direction with decisions proposed; not implemented, not validated.
Nothing here is qualified. Every decision below is a provisional hypothesis stated
so it can be argued with, not a settled contract.

## Purpose

Store versioned universe recovery points independently of live replicas. Support
configuration, required image references or content, persistent volumes, database
state and optional memory checkpoints.

**Replication is not backup.** It propagates deletion and corruption faithfully and
at speed. The manager's replicated history, the migration chain and this service
answer three different questions: where a universe *is*, how it *moves*, and what it
*was*.

## What the canon already decides

Two rules of `SHAPER-OS-V1.14/software/RULES.md` bind this service. They are not
re-decided here; they are requirements, and several were written after real
incidents.

**Rule 16 — the five levels.** Container, volumes, database, git, off-site. "This
set is enough. Missing a level is a hole." Three consequences the design must carry:

- Level 2 is the **volumes, not the overlay**. `nosav/`, caches, image layers and
  `node_modules` are excluded. `app/` is code in git and is *not* a substitute for a
  volume backup.
- Level 3 is a **real dump**, not a live volume tar. A tar of a running database's
  files is not crash-consistent and must never be presented as the database.
- Level 4 is git, and **git is never treated as a data backup**.

**Rule 12 — archive hygiene.** Every clause is a failure mode this service must make
impossible rather than merely avoid:

- The key that opens the coffer does not travel with the coffer.
- The backup's encryption key is **its own key**, refused if it equals the vault
  master key, and it reaches the tool through the environment, never as an argument
  readable by every process on the host.
- A dump that was not taken is **announced**, never written empty. A dump whose
  client fails, or whose output is empty, fails the backup.
- **The archive command's failure is the backup's failure.** A status line is
  printed after the archive exists, has a size and has a checksum, or not at all.
- A failure *after* the archive is complete **keeps the archive**. Housekeeping that
  cannot run is a reported failure over a surviving archive, never a deletion.
- A failed dump **leaves nothing behind**: written under `.part`, renamed only once
  it has a size.
- `.env` is excluded in **every spelling** (`.env*` and `*.env`), never the bare name.
- How the tool is called is **proven with a recorder** on a PATH built from scratch:
  which client name, where the password travels, what a failing or empty client does
  to the backup.

Rule 13's WireGuard mesh is the available private transport. Rule 20's closed-loop
quality gate applies: the delivered interaction is exercised, not simulated.

## What we take from Proxmox Backup Server, and what we do not

PBS solved the storage economics of this problem well, and there is no reason to
reinvent them.

**Taken.**

- A **content-addressed, deduplicating chunk store.** Universes are near-identical
  by construction — same base image, same brick set, same layout — so dedup across
  universes is not an optimisation, it is the entire economy of the thing.
- **Incremental forever.** No periodic fulls. PodMesh has no dirty-bitmap equivalent,
  so incrementality comes from content addressing alone: re-chunk, transfer only the
  chunks the store does not have. Slower to read than PBS's bitmap, identical in what
  it stores.
- **Verify jobs.** Re-read and re-checksum stored chunks on a schedule, independently
  of any backup or restore. This is the same doctrine the rest of PodMesh already
  follows: proof is read from outside the producer, and a job that reported success
  is not evidence that the bytes are still there.
- **Datastore-to-datastore sync** as the mechanism for Rule 16's level 5.
- **Namespaces** for separating tenants and fractal levels inside one datastore.

**Deliberately not taken.**

- **PBS's push model.** See D1.
- **Retention purely by schedule.** See D4.
- Tape, and live-restore-from-the-datastore. Out of scope; neither is on the path to
  anything Xavier needs.

## The problem PBS does not have

A PBS backup is a disk image and a config file. **A PodMesh recovery point is not a
disk.** It is a set of pieces that are only meaningful together:

| Piece | Rule 16 level | Notes |
| --- | --- | --- |
| Universe identity and its PodMesh journal facts | — | the UUID, the creation operation, ownership |
| Container configuration | — | command, labels, network, mounts, resources |
| Image identity, and content where the registry is not durable | 4 / 5 | a digest, plus the layers when nothing else holds them |
| Persistent volumes | 2 | volumes only, `nosav/` excluded |
| Database dumps | 3 | per Rule 12's dump laws |
| Memory checkpoint | optional | only where the runtime permits it |

The manifest must therefore state **what is consistent with what**. A memory
checkpoint is valid only against the exact disk state it was taken with; a database
dump is consistent with itself but not necessarily with the volume tar taken a minute
later. Every recovery point declares a **consistency class**:

- **crash-consistent** — the pieces were captured from a running universe with no
  quiescing. Restorable; the application must survive it as it would survive a power
  cut.
- **application-consistent** — the database was dumped through its own client and the
  volumes captured around it, with the ordering recorded.
- **memory-coherent** — a checkpoint plus the exact disk state it was taken against,
  bound together, restorable to a running process.

A recovery point that cannot achieve a class **says which class it is**, and a
restore never silently upgrades one to another.

## Decisions proposed

**D1 — the backup server pulls, and has no inbound listener.**
It holds every universe's data, which makes it the highest-value target in the
constellation. The canon's security direction is that what has power has no front
door. So the backup server opens outbound connections to hosts, reads through a
narrow read-only capture surface, and stores. A compromised host cannot reach it,
cannot enumerate other universes' backups, and cannot delete anything. This inverts
PBS, where the client pushes and the datastore listens, and it is the single most
consequential divergence. *Cost:* the backup server holds credentials to reach every
host, and a scheduler lives on the protected side.

**D2 — content-addressed chunks, deduplicated within one key domain.**
Digest of the *plaintext* chunk, so dedup works; the chunk stored encrypted. A holder
of the index can therefore learn which known chunks a universe contains. That is
PBS's trade too, it is acceptable here, and it must be written down rather than
discovered later.

**D3 — encryption happens on the host, before transfer.**
The server stores opaque chunks it cannot read. Rule 12 governs the key: its own key,
never the vault's, never in the archive, never on a command line. Key escrow and
recovery are part of the first lot, not an afterthought — an unrecoverable key is a
backup that does not exist.

**D4 — retention is a schedule for *candidacy*, and proof for *removal*.**
PBS prunes by keep-last/daily/weekly/monthly. The operator's own garbage-collection
contract, written for the migration collector, says age never justifies a collection
— only proof does. Both survive if they are given different jobs: the retention
schedule selects candidates, an **evidence hold** blocks any candidate touched by an
incident, and a recovery point is removed only once a **newer recovery point covering
the same universe has been verified**. Chunks are then swept by mark-and-sweep
against surviving manifests, never by age.

**D5 — a restore is proven, periodically, from outside.**
A backup job that reported success is not evidence. The service restores to a scratch
target on a schedule and verifies the result from outside the restorer: the universe
starts, its data hashes match, its database answers. Measured recovery time and
measured data-loss bounds are outputs of that test, not estimates.

**D6 — a recovery point declares its completeness against Rule 16.**
It names, per level, whether that level is present, deliberately absent, or *failed*.
"Missing a level is a hole", so the hole is on the manifest where a human sees it,
not implied by an absence.

**D7 — the first lot mirrors the workload shape migration already qualifies.**
Alpine, musl, network-disabled, mount-free, on the existing Debian 13 hosts. Nothing
is learned by discovering volume and network problems inside the first backup lot
that the migration work has not solved either.

## Delivery order

Each lot ends with evidence read from outside and an honest statement of what it does
not cover.

1. **B1 — datastore and one round trip.** Chunk store, manifest, the pull surface, and
   a verified restore of a stopped, mount-free universe's configuration and filesystem
   to a *different* host. Rule 12's hygiene guards implemented and proven with the
   recorder. Key generation, escrow and recovery. This is the lot that makes
   everything else meaningful.
2. **B2 — volumes.** Rule 16 level 2 with the `nosav/` exclusion and the `.env*`
   exclusion in every spelling.
3. **B3 — databases.** Level 3 under Rule 12's dump laws: the right client name, the
   password in the environment, `SKIP` announced, `.part` then rename, empty output
   fails the backup.
4. **B4 — retention, holds and sweep.** D4, reusing the vocabulary of
   `docs/GARBAGE-COLLECTION.md` rather than inventing a second one.
5. **B5 — off-site.** Level 5 as datastore-to-datastore sync, pulled by the remote
   side, encrypted, cold.
6. **B6 — memory-coherent points.** Only where the runtime permits, bound to the exact
   disk state, reusing the migration chain's checkpoint machinery and its proven
   limits.
7. **Later.** Scheduling policy, the human interface, and the container/VM level 1
   snapshot integration where a hypervisor provides it.

## What must be proven, not claimed

- A restore onto a **different** host, verified from outside. A successful backup job
  alone is insufficient, and this is already the standing requirement in the delivery
  checklist.
- That a **destroyed** universe can be brought back — not that a copy exists.
- That the key can be **recovered** by the operator from their own key material, with
  the coffer alone proving nothing.
- That a **partially failed** capture is reported as failed and keeps whatever it
  completed.
- Each deployment mode separately: standalone Debian host, container, VM or VPS, and
  ShaperOS universe. Host installation does not prove container operation.
- Measured recovery time and measured data-loss bounds, per level and per consistency
  class.

## Deployment portability

Unchanged from the original direction and still binding. Standalone operation without
ShaperOS is mandatory across supported host types; ShaperOS deployment is the
operator's preference for internal use and must not become a hard dependency. The
same backup and restore contracts apply in both modes.

Separate the storage server from host-side capture: receiving and retaining backups
does not require Podman or CRIU locally. Host-side capture of filesystem, application
and optional process state does. Recovery with memory has further kernel and runtime
constraints, and ordinary backup storage must not inherit them.
