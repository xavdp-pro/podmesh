# Migration protocol: transfer authority, source exclusion and recovery

Status: provisional design decision, 2026-09-11. Taken by the implementing tandem
(Claude Code, Opus 5) under the operator's standing mandate and his instruction to
take over without per-step approval. Not implemented. Xavier may amend or revoke it
at any time; implementation lots record every deviation in this file.

This completes the source-side checkpoint milestone ([REVIEW-MIGRATION-SOURCE.md](REVIEW-MIGRATION-SOURCE.md),
[LOCAL-API.md](LOCAL-API.md)). It answers the open question left by that milestone:
when may a checkpointed source run again, who authorizes a transfer, and what proves
that exactly one host holds an active universe.

## Scope

- Serial migration of one network-disabled, mount-free, journal-owned universe between
  two PodMesh hosts, default rootful store, packaged podmesh-vzcriu runtime.
- Requested by the tandem (standalone mode) or by the maker acting on its governor's
  ledger row (ShaperOS mode). PodMesh never decides to migrate and never chooses a host.
- Not covered: high availability, automatic failover, partition takeover, networked
  universes, volumes, periodic replication, an authenticated remote API.

## Actors

| Actor | Holds | Never |
| --- | --- | --- |
| Requester (tandem or maker) | the decision and its `authorization_ref` | bypasses a PodMesh refusal |
| Transport controller | bytes and documents in transit (SSH/scp initially) | grants authority; its copies are verified on arrival |
| Source PodMesh | the source journal, reservation and artifacts | restarts the source implicitly |
| Destination PodMesh | the destination journal, restore claim and restored container | restores without a verified handoff |

No PodMesh-to-PodMesh network connection is added: each service keeps its root-only
local socket. Direction stays outward from whoever holds power.

## Invariants

1. **One active host per universe.** The only event that moves activity is a verified
   destination restore bound to a source transfer authorization.
2. **Authorization before bytes leave.** The source journal records the authorization
   (destination, archive and manifest hashes, requester reference) before the artifacts
   are placed in its outbox. No authorization, no export.
3. **No local release after authorization.** Once an authorization exists, only a
   verified destination outcome bound to it ends the source reservation. An unreachable
   destination leaves the source held. A manual break-glass path is OPEN, not designed here.
4. **Restore requires the handoff.** The destination refuses unless the handoff names
   this host, the archive bytes hash to the authorized value, the universe is absent and
   unclaimed locally, and runtime and image preflight pass.
5. **Nothing restarts implicitly.** No failure, release, abandonment or retry starts an
   application on either host.
6. **Typed, idempotent operations.** Every step has a stable operation ID on the host
   where it acts and follows the existing replay and re-evaluation contract.
7. **Proof from outside the producer.** Memory continuity is established by an observer
   comparing a memory-only token and a progress counter before checkpoint and after
   restore, never by the restore command's exit code.

## Documents

Documents travel through fixed directories under each service's state directory:
`outbox/<authorization_id>/` (written only by the service) and
`inbox/<authorization_id>/` (written by the transport controller as root, read by the
service). File names are fixed; requests never carry paths. The 4 KiB request limit
therefore never has to carry a document.

- **Handoff** `podmesh-transfer-handoff/1`: authorization ID, universe UUID, source
  container ID, image ID, source and destination host UUIDs, checkpoint operation ID,
  archive bytes and SHA-256, manifest SHA-256, runtime git ID and binary SHA-256, kernel
  release, issue time, requester `authorization_ref`. Identified by its own SHA-256.
- **Outcome** `podmesh-transfer-outcome/1`: handoff SHA-256, destination operation ID,
  `result` = `restored` or `not_restored`, restored container ID when restored, the
  destination's fresh observation, timestamps.

A document is not a credential. The consuming service checks it against its own journal
and the bytes it can hash. Root on either host can forge journals; that threat is out of
scope, as it already is for every local operation.

## Source reservation states

Existing: `reserved → checkpointing → checkpointed`, and `checkpoint_failed`.

| From | To | Operation | Condition |
| --- | --- | --- | --- |
| `checkpointed` | `transfer_authorized` | `migration_authorize_transfer` | artifacts re-hash to the recorded values; same reserved container, still checkpointed and not running |
| `transfer_authorized` | `transferred` | `migration_complete_transfer` | inbox outcome `restored`, bound to this handoff SHA-256 and destination |
| `transfer_authorized` | `checkpointed` | `migration_complete_transfer` | inbox outcome `not_restored`, bound to this handoff; the authorization is recorded as ended |
| `checkpointed`, `checkpoint_failed` | `released` | `migration_release` | no authorization was ever issued for this reservation; fresh observation of the same container, not running |
| `reserved`, `checkpointing`, `checkpoint_failed` with the container absent | `abandoned` | `migration_abandon` | no authorization ever issued; artifacts preserved |
| `transferred` | — | `migration_retire_source` | removes only the stopped reserved container and its kept checkpoint images; evidence directory kept |

`released` lifts the generic-operation gate. It starts nothing. From `released`, the
caller chooses explicitly between `migration_restore_local` (resume the checkpointed
memory from the kept checkpoint or the archive) and an ordinary `start`, whose result
states that memory was not restored. `abandoned` and `transferred` keep refusing `create`
with that universe UUID on this host, except through a verified handoff restore.

## Operations

### Source host

- `migration_authorize_transfer` — binding: checkpoint operation ID and the destination
  recorded in the reservation. Writes the authorization row first, then the outbox
  (archive, manifest, handoff). Replays return the same handoff.
- `migration_complete_transfer` — binding: authorization ID. Reads the inbox outcome and
  applies the transitions above; any mismatch is refused without state change.
- `migration_retire_source`, `migration_release`, `migration_abandon` — as in the table.
- `migration_restore_local` — lot M3, after the destination path is proven.

### Destination host

- `migration_destination_preflight` — read-only, from the inbox handoff: destination is
  this host; no container `podmesh-<uuid>` and no active claim for the UUID (a local
  `transferred` history does not block a return trip); image present with the same ID;
  runtime git ID and binary hash equal to the handoff; kernel release equal (a mismatch
  is a blocker in the qualified scope); archive present and hashing to the handoff; space.
- `migration_restore` — binding: authorization ID. Repeats the preflight, persists a
  restore claim, then runs the restore in its own transient scope with the private
  runtime first on PATH. Verified only when Podman shows the universe container running
  with the handoff's universe label. It records destination ownership (below) and writes
  the outcome to its outbox. A failure before Podman starts restoring writes
  `not_restored`. A failure after that leaves the claim held and writes no outcome
  until `migration_restore_abort` has removed only a non-running container created by
  this claim and verified its absence.

### Destination ownership

The restored container keeps the source journal's creation label, which the destination
journal does not know. Ownership on the destination is therefore the verified
`migration_restore` operation binding the universe UUID and the restored container ID.
`owned()` accepts a verified `create`, `clone` or `migration_restore` whose container ID
matches. A label alone stays insufficient.

## Acceptance for the destination lot (M2)

Development service on the destination host, isolated like the source milestone.

- Forward migration of a memory fixture holding a random token and a counter: same
  token, counter continues from its checkpointed value and progresses; source
  `transferred` and stopped; destination running and owned; stop, start and delete work
  on the destination through the API.
- Return trip to the original host, same universe identity, same checks.
- Refused without effect: restore without handoff, wrong destination, tampered archive,
  occupied name or UUID, absent image, runtime mismatch, release after authorization,
  completion with an outcome bound to another handoff.
- Replays: authorize and restore are historical when retried, demonstrated directly. Completion
  rides the same generic replay path; its repeat is not separately demonstrated.
- Interruption: service killed during `migration_restore`; the retry reconciles to one
  container and one outcome.
- Unrelated containers untouched; every Podman event on managed universes falls inside
  an API window.

## Deviations recorded by the destination implementation (lot M2)

Implemented on 2026-09-11 by Claude Code (Opus 5) in the development tree and exercised on two lab hosts.
Each entry is either a decision this protocol did not settle or a difference from what it describes. Xavier
may amend any of them. Lot M3 (`migration_release`, `migration_abandon`, `migration_restore_local`) is not
implemented, so a reservation still has no local way out.

1. **Refusals before a claim write no outcome.** The protocol says a failure before Podman starts restoring
   writes `not_restored`. A refusal raised by the repeated preflight happens before anything is claimed, leaves
   no trace and stays retryable with the same operation ID, which is what a re-copied archive or a newly loaded
   image needs. Only a claim that was persisted and whose command could not be started is closed `not_restored`.
2. **`migration_restore_abort` also declines an authorization this host never claimed.** Otherwise a destination
   that refuses a handoff (absent image, different kernel) leaves the source held indefinitely, since only a
   verified destination outcome ends an authorization. The decline is recorded as a closed claim before the
   outcome is written, so this host can never restore that authorization afterwards.
3. **A verified restore archives an earlier `transferred` reservation** into a history table. The reservation
   table is keyed by universe and `transferred` keeps refusing generic operations; without archiving, a returned
   universe could neither be operated nor checkpointed again on its original host. `migration_status` reports
   the archived rows.
4. **Ownership is withdrawn from a container transferred away.** `owned()` accepts a verified create, clone or
   `migration_restore` for the observed container ID, and additionally refuses a container ID this host recorded
   as transferred, so that restoring an old archive out of band under its original ID cannot regain ownership
   through the original creation.
5. **The restore uses `--keep` and `--name podmesh-<uuid>`.** `--keep` preserves the CRIU restore log that the
   verification requires: selecting the runtime through `PATH` does not prove which engine restored the
   universe. The kept files stay in the restored container's storage until it is removed or checkpointed again,
   and a later export from that container may carry image files left by the previous restore. `--name` gives
   each hop a new container ID, recorded in the outcome and the journal.
6. **`INVOCATION_ID` is removed inside the transient scope, not only around it.** `systemd-run --scope` sets it
   for the command it runs (observed with systemd 257), and Podman then leaves conmon inside that scope, which
   would stay active for the life of the restored universe and defeat the scope-completion check. The checkpoint
   command strips it the same way, for consistency.
7. **The destination restores a private copy of the archive.** The inbox bytes are hashed, copied into the
   operation's directory and re-hashed there, and Podman reads that copy: a transport controller writing into
   the inbox during a restore cannot change what is restored.
8. **The handoff carries the dump-time runtime and kernel**, taken from the checkpoint manifest rather than from
   a fresh observation at authorization, because they describe the engine that produced the archive.
9. **Authorization IDs are service-issued UUIDs**, not caller-chosen operation IDs, and they name the delivery
   directories.
10. **Completion is refused while the source container is running or replaced**, rather than recording a
    destination outcome over an unexplained local activity.
11. **Retirement keeps the reservation in `transferred`** and records the retirement in its detail; there is no
    `retired` state. An already absent container is a verified no-op, and a source that is no longer
    checkpointed is refused rather than removed.
12. **The destination preflight adds two checks the table above leaves implicit**: the manifest must agree with
    the handoff field by field, and the archive's own `config.dump` must name the handoff's source container,
    image and universe label.
13. **An unresolved restore claim gates generic operations** exactly as a reservation does, because such a claim
    may hold a container the destination created.
14. **`migration_status` reports authorizations, restore claims and archived reservations** beside the
    reservation, so one read-only call shows both sides of a migration on either host.

## Still open

- Break-glass release when the destination is unreachable or its journal is lost.
- Moving network identity and addresses (P11–P13).
- Transport by makers, authenticated remote operations and the manager's role.
- Periodic replication and controlled failover (P10, P16, P17).
