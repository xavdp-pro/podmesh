# Local API contract

The experimental daemon accepts one newline-terminated JSON request per Unix socket connection and returns one JSON response line. The default endpoint is `/run/podmesh/api.sock`, restricted to root. The CLI is a client of this same endpoint.

## Discover before acting

```sh
sudo podmesh capabilities
sudo podmesh identity
sudo podmesh inventory
sudo podmesh observations
```

Capabilities describe the installed build. Do not infer that planned operations are available from the roadmap.

**The schemas.** `capabilities` also carries `schemas` (`schema_version`
`podmesh-operation-schema/1`): for every advertised operation, its `kind` — `read` (no journal row,
no effect), `universe` (a journaled mutation naming one universe), `host` (a journaled mutation of
this host), `tool` (one step of a chain a workstation tool drives across hosts) — its `gate` (`none`,
`lease`, `reservation`, or both), a description, and its `fields`, each with a `type` (`string`,
`integer`, `number`, `boolean`, `enum` with `values`, `uuid`, `string[]`, `uuid[]`, `object[]`,
`object`), `required`, and the `min`/`max` the daemon enforces. An operation whose fields are not
described yet says `fields: null`. This is what the console draws its forms from and what an agent
reads before asking; `tests/check-capabilities-schema.py` holds the stated bounds to the daemon's
refusals.

## Typed mutation requests

Pass a JSON request file as the second CLI argument:

```sh
sudo podmesh create request.json
```

A creation request contains:

```json
{
  "operation_id": "an-explicit-unique-operation-id",
  "universe_uuid": "a-valid-new-universe-uuid",
  "authorization_ref": "operator-approved-lab-work",
  "image": "sha256:FULL_LOCAL_IMAGE_ID",
  "command": ["sleep", "300"]
}
```

The UUID and image above are placeholders, not executable examples. The CLI sets the operation field from its first argument. The initial creation operation produces a stopped container; it does not start the application. Image pulling is not implicit. In the packaged releases the container has networking disabled; in the development tree `create` requires a `network_profile` (`isolated`, the same `--network=none`, or `managed`), see the network section below and `UNIVERSE-NETWORK-CONTRACT.md`.

Clone requests additionally identify `source_uuid` and use a new target `universe_uuid`. Deletion identifies the target universe and must not be used to imply a stop operation. Consult installed capabilities and the release's tested scope for exact restrictions.

## Ownership

Delete, start, stop and clone sources act only on a container named `podmesh-<universe_uuid>` whose `io.podmesh.creation-operation` label names a **verified** create or clone operation in this host's journal, for the same universe UUID and the same container ID. A PodMesh label alone, a label borrowing another universe's operation, or a container replaced out of band under the same name is refused. A container with that name but without the universe label is refused as unmanaged.

When the target container is absent, delete removes only snapshot images labelled for that universe whose clone operation is recorded in the journal for that universe and source, and, once verified, for the same image and source container. Other images are retained and reported.

## Start

```json
{"operation_id": "...", "universe_uuid": "...", "authorization_ref": "...", "observe_seconds": 2}
```

`observe_seconds` is optional (integer 0–30, default 2). Start requires a recorded universe in state `created`, `exited` or `stopped`; a running universe returns `action: none_already_running` without restarting it; other states (for example paused) are refused. After `podman start`, the service observes the container until it stops running or the window ends, and reports `observed_state`, `running`, `started_at`, and, when not running, `finished_at` and `exit_code`. `running: true` means running when observed at the end of the window, not afterwards. A short-lived application is reported as not running with its exit code. A runtime failure (for example a missing executable) is `ok: false` with the observed state in `details`.

## Stop

```json
{"operation_id": "...", "universe_uuid": "...", "authorization_ref": "...", "timeout_seconds": 10, "on_timeout": "kill"}
```

Both fields are required; there is no default escalation.

- `on_timeout: "kill"` runs `podman stop --time timeout_seconds`: the container's stop signal, then SIGKILL if it is still running after the timeout. `forced` is true when Podman reports that escalation (it reports it as a warning; exit code 137 is typical).
- `on_timeout: "leave_running"` sends only the container's stop signal and waits `timeout_seconds`. If the application is still running, the result is `ok: false`, the container is left running, and no SIGKILL is sent.

A universe that is already stopped returns `action: none_already_stopped` without sending a signal. Stop never removes the container, its filesystem or volumes.

## Storage

```json
{"operation": "storage_status", "operation_id": "...", "authorization_ref": "..."}
```

Read-only and host-wide: what carries Podman's storage (`graph_root`, its `mount` from findmnt,
the `backend` — `zfs`, `btrfs`, `lvm-thin`, `lvm`, `plain` — and whether it is `dedicated`, that is
not the system's root filesystem), the filesystem's sizes from df, and the operator's rule applied
(2026-09-16): a universe's space may `growth: possible` only on a dedicated LVM, ZFS or Btrfs
volume; on a filesystem shared with the system, or a dedicated one that does not know how to grow,
it is `refused` with the reason. Universe volumes, `volume_declare` and `volume_grow` come with the
dedicated storage; today a universe keeps its data in its container's layer, and the result says so.
On the laboratory hosts, whose storage sits on the ext4 root, the answer is `refused`.

## Pause and resume

```json
{"operation": "pause", "operation_id": "...", "universe_uuid": "...", "authorization_ref": "..."}
{"operation": "resume", "operation_id": "...", "universe_uuid": "...", "authorization_ref": "..."}
```

`pause` freezes every process of a running universe (`podman pause`): its memory and its
address stay, nothing is scheduled, and the result is verified from Podman (`paused`). A paused
universe is already stopped for every purpose of a second writer, so `pause` is never gated by the
activation lease; a universe already paused answers `action: none_already_paused` and nothing is
sent; any other state is refused at the contract (a stopped universe is not paused, it is stopped).

`resume` thaws a paused universe (`podman unpause`). It leaves a running writer behind, so it passes
**the same gate as `start`**: under an activation policy it is refused without a live lease on this
host. A running universe answers `action: none_already_running`; a stopped one is refused, since a
stopped universe is started, not resumed. A migration reservation blocks both, as it blocks `start`.

## Resources

```json
{"operation": "resources", "operation_id": "...", "universe_uuid": "...", "authorization_ref": "...", "memory_bytes": 134217728, "cpus": 0.5}
```

Sets the memory limit and the CPU allowance of a universe (`podman update`), one or both:
`memory_bytes` from 32 MiB to this host's total memory (swap is limited to the same value, so a
limit is a limit), `cpus` from 0.1 to this host's number of cores, fractions allowed, rounded to a
hundredth. On a running or paused universe the kernel applies them to its cgroup at once, and the
result reads them back from there — `kernel.memory_max_bytes`, `kernel.cpu_max`, `kernel.cpus` from
the unified hierarchy — and refuses when Podman's record or the kernel disagrees with what was
asked (`verification: "kernel"`). On a universe created and never started, Podman records them for
its first start (`verification: "recorded"`). On an exited universe, measured on Podman 5.4.2, the
update is accepted and the next start applies it, but Podman's record shown by inspect keeps the
previous values until that start: the result says `verification: "deferred"` and its note says it
is not verified here. Out-of-range values, and a request naming neither field, are refused before
anything is sent. A migration reservation blocks it. Removing a limit is not offered in this
version.

## Timeouts, interruption and cancellation

There is no cancellation operation. The service handles one request at a time; a client disconnect or client timeout does not cancel the operation, which continues to completion. Each Podman call is bounded (30 s; commits 300 s; stop `timeout_seconds` + 30 s); a call exceeding its bound is terminated and the operation fails with the re-observed state, not an assumed one. If the service itself is killed, the operation remains `pending`. Every attempt is recorded in the `operation_attempts` table.

Retrying a pending or failed operation re-evaluates the observed state. To avoid repeating an effect, a retried **start** does not start again if the container has started since the first attempt began, and a retried **stop** is refused if the container may have been started after the first attempt began (it would stop a newer run). Timestamps have one-second resolution; the comparison is conservative. Killing the service during a stop wait interrupts `podman stop` before any escalation: Podman then records the container as `stopping` while the application is still alive. PodMesh reports such a container as `running: true` with `state: stopping` and a note, refuses to start it, and accepts a stop (a retry of the interrupted operation or a new one) to complete it under that request's declared timeout behavior.

Started containers keep running when the PodMesh service stops, restarts, or is upgraded or removed: Podman's container monitor (conmon) runs in its own `libpod-conmon` scope, not in the service cgroup.

## Results and retries

Check both CLI exit status and JSON `ok`. Keep the same operation ID and byte-equivalent semantic request when retrying. Reusing an operation ID for a different request fails. A verified operation is never executed again: the response is `replayed: true, historical: true`, with `original_result` as persisted when it was verified, `verified_at` when recorded, and `current`, a fresh Podman observation of the universe container, plus `current_matches_recorded_container`. The historical result may contradict current state; use `current`, inventory or independent inspection for present facts.

A dropped connection is an uncertain outcome, not proof that no action occurred. Preserve the request and inspect or retry it according to the operation contract. Do not generate a new operation ID blindly after a timeout.

## Experimental: source-side migration preparation (packaged since 0.1.0~experimental5)

These operations prepare and checkpoint a migration **source**. Transfer of authority to another host and the destination side are the separate operations documented in the next section; the recovery paths of a reservation that never left this host — release, abandonment and local restore — are documented after them. See MIGRATION-INTEGRATION.md and MIGRATION-PROTOCOL.md for the protocol and its remaining gaps.

Requests carry the full binding: `operation_id`, `universe_uuid`, `authorization_ref`, `container_id` (full 64-character ID), `image` (full `sha256:` image ID), `source_host_uuid` (must be this host) and `destination_host_uuid` (recorded, not contacted; must differ from the source).

- `migration_preflight` reports `compatible` and `blockers` with fresh facts and has no effect: no reservation, signal, suspension or artifact. Like every operation, repeating the same operation ID returns the historical report; a fresh assessment uses a new operation ID. Blockers include: not owned by this host's journal, identity mismatch, not running, network mode other than `none`, volumes or bind mounts, privileged or TTY containers, a frozen cgroup, more than 64 processes, container memory above 1 GiB or unreadable (a dump still running at the 300 s bound is killed, which destroys the application), any process that is not musl-based (glibc registers rseq, which the qualified CRIU 3.15 predates; static binaries cannot be identified), a Podman graph root other than the default store, insufficient space in the state directory or the graph root, an existing reservation, and a private runtime whose binary, wrapper or shim differs from the pinned SHA-256 or whose `criu check` fails.
- `migration_checkpoint` repeats every check immediately before acting and refuses before any suspension if one fails. It then persists a reservation, creates `migrations/<operation_id>/` under the service state directory (0700, files 0600), and runs `podman container checkpoint --export --compress=zstd --keep --file-locks --print-stats` on the reserved container ID (not the universe name) with `PATH` beginning at the packaged private shim `/opt/podmesh-vzcriu-kit/bin`, inside its own transient systemd scope `podmesh-checkpoint-<operation_id>.scope`, bounded to 300 s. The distribution CRIU is never used. The result is verified only if Podman reports the same container `Checkpointed` and stopped, the archive lists the expected entries, and the copied CRIU log shows a successful dump by `v3.15.5.3`. The directory keeps `checkpoint.tar.zst`, `manifest.json`, `dump.log`, `preflight.json`, per-attempt stdout/stderr and failure records; the result carries the archive and manifest SHA-256.
- `migration_status` (read-only, `universe_uuid` only) returns the reservation, a fresh observation, a re-hash of the preserved archive and manifest, the transfer authorizations, restore claims and archived reservations, and a `recovery` object saying which of `migration_release`, `migration_abandon` and `migration_restore_local` the observed state permits, each with its blockers. The older `release` key keeps its `preconditions_observed` and now reports the same verdict.

A reservation blocks `create`, `start`, `delete` and `clone` for that universe (as target or source); `stop` remains available: it cannot run, replace or remove the source, but stopping a source whose checkpoint failed ends its process and memory state. A `released` reservation blocks nothing — that is what releasing means — while `abandoned` and `transferred` keep refusing all four. Earlier package versions running on the same journal after a rollback do not enforce reservations.

Retries: a verified checkpoint is never captured again; its replay is historical and adds `current_artifacts` with a fresh re-hash. A pending or failed checkpoint is re-evaluated: if its scope has not finished (including activating or deactivating) or its state cannot be queried, the retry is refused; if Podman shows the reserved container checkpointed after the reservation, the existing archive is finalized without capture (`finalized_after_interruption: true`); if the same process is still running and was never checkpointed, it is checkpointed again; anything else is refused with the observed state, and nothing is restarted. Killing CRIU itself mid-dump was observed to destroy the application without an archive; the separate scope protects against a service crash, not against CRIU or host failure.

## Experimental: serial migration between two hosts (packaged since 0.1.0~experimental5)

The operations above prepare and checkpoint a universe. The ones below move the authority over that universe to one named destination host, and back. The protocol and the deviations this implementation recorded are in MIGRATION-PROTOCOL.md. Nothing here is packaged or qualified for ordinary workloads: a reservation is not fencing, and direct administration bypasses it.

Documents travel through fixed directories under the service state directory: `outbox/<authorization_id>/` is written only by the service, `inbox/<authorization_id>/` by the transport controller as root. Requests carry an `authorization_id` and never a path, so the 4 KiB request limit never has to carry a document. A document is not a credential: every value in it is checked against this host's journal and against bytes the service hashes itself. Root on either host can forge documents and journals; that threat is out of scope, as it already is for every local operation.

### Source

- `migration_authorize_transfer` (`checkpoint_operation_id`, `destination_host_uuid`) moves a `checkpointed` reservation to `transfer_authorized`. It re-hashes the preserved archive and manifest, observes that the reserved container is still checkpointed and not running, and checks the manifest bindings. The authorization row, with a service-issued `authorization_id` and the exact handoff bytes, is written **before** any artifact leaves the host; the outbox then receives `checkpoint.tar.zst`, `manifest.json` and `handoff.json` (`podmesh-transfer-handoff/1`: universe, source container, image, both host UUIDs, checkpoint operation, archive bytes and SHA-256, manifest SHA-256, the runtime git ID and binary SHA-256 and the kernel release recorded when the dump was finalized, issue time and the requester's `authorization_ref`). A replay returns the same handoff with a fresh re-hash of the outbox; a second authorization of the same reservation is refused.
- `migration_complete_transfer` (`authorization_id`) reads `inbox/<authorization_id>/outcome.json` and applies it: `restored` makes the reservation `transferred`; `not_restored` returns it to `checkpointed` and records the authorization as ended. The state machine then allows a new authorization for the same recorded destination; that retry is not exercised by the current tests. The outcome must be a `podmesh-transfer-outcome/1` document bound to this authorization's handoff SHA-256, this universe, this source host and the recorded destination. Any mismatch is refused without a state change, as is a source container that is running or has been replaced.
- `migration_retire_source` (`authorization_id`) removes, from a `transferred` reservation, only the stopped container that is still the reserved one and still checkpointed, together with the checkpoint files Podman kept for it. The reservation stays `transferred` and keeps refusing generic operations and `create` for that universe UUID; the evidence directory is kept. An already absent container is a verified no-op.

### Recovery on the source host

These three end a reservation whose migration is not going to happen. Each names the checkpoint whose
reservation it acts on (`checkpoint_operation_id`), so a request can never resolve to a reservation the
caller did not mean, and none of them starts an application. **None of them is possible once a transfer
authorization has been issued for the reservation**, whatever became of it: from that moment only a
verified destination outcome bound to it can end the reservation, and an unreachable destination leaves
the source held. There is still no break-glass path.

- `migration_release` moves a `checkpointed` or `checkpoint_failed` reservation to `released`, after
  observing that the container under the universe name is still the reserved one and is not running. It
  lifts the generic-operation gate — `create`, `start`, `delete` and `clone` work again for that universe
  — and starts nothing. The preserved archive and the checkpoint files Podman kept are untouched, so the
  caller then chooses explicitly between `migration_restore_local` and an ordinary `start`.
- `migration_abandon` records that a reservation will never be completed, from `reserved`,
  `checkpointing`, `checkpoint_failed` or `checkpointed`, and only when the reserved container is absent
  (not under the universe name, not under any other name). The artifacts are kept and verifiable, and the
  universe UUID keeps refusing `create`, `start`, `delete` and `clone` on this host: only a verified
  restore of a handoff from another host brings that universe back. Abandoning a `checkpointed`
  reservation therefore gives up the local restore of its preserved memory; that is the point of the
  operation, and `migration_release` is the path for a reservation whose container is still there.
- `migration_restore_local` resumes the checkpointed memory on this host, from `released`. It prefers the
  checkpoint files Podman kept for the reserved container — an in-place restore, which keeps the same
  container ID — and falls back to the preserved archive when those files are gone, which produces a new
  container ID under the same universe name. It is verified exactly like a destination restore: Podman
  must show the container running and `Restored`, restored after this operation began, from the recorded
  image, network-disabled and mount-free, **and** the preserved CRIU restore log must show a successful
  restore by the qualified runtime. Memory continuity itself is established by an observer outside the
  universe, never by this result. A verified local restore then archives the reservation into the history
  table, so the universe is fully operable again: it can be checkpointed, started, stopped and deleted.
  A container restored from the archive keeps the original creation label with a new ID, so `owned()`
  accepts the verified `migration_restore_local` that binds the universe UUID to it, as it does for an
  imported restore.

A universe whose container was started out of band after its checkpoint is refused by
`migration_restore_local`: Podman no longer reports it as checkpointed, and restoring stale memory over a
container that has since run would be wrong. Deleting that container makes the preserved archive
restorable under the same name again. An ordinary `start` of a released universe says so in its result:
`memory_restored: false`, with a note that the application began afresh. A new `migration_checkpoint` of
a universe whose reservation is `released` supersedes it: the released row is archived with its history
and the new checkpoint reserves afresh.

### Destination

- `migration_destination_preflight` (`authorization_id`) is read-only and reports `compatible` and `blockers`: the inbox handoff names this host and another source; no container `podmesh-<uuid>`, no container carrying the universe label, no active reservation (a local `transferred` history does not block a return trip) and no other unresolved restore claim; the image is present locally (PodMesh never pulls); the runtime binary hash, the runtime git ID and the kernel release equal the handoff's; the archive and manifest hash to the handoff and agree with it field by field; the archive's own `config.dump` names the handoff's source container, image and universe label, and the archive lists the expected checkpoint entries; both the state directory and the Podman graph root have room. Repeating the operation ID returns the historical report.
- `migration_restore` (`authorization_id`) repeats that assessment, copies the archive into its own operation directory and re-hashes the copy, persists a restore claim, then runs `podman container restore --import --name podmesh-<uuid> --keep --file-locks --print-stats` in its own transient scope with the private runtime first on `PATH`. It is verified only when Podman shows that container running and `Restored`, created and restored after the claim, carrying the handoff's universe label, from the handoff image, network-disabled and mount-free, **and** the preserved CRIU restore log shows a successful restore by the qualified runtime. A container whose cgroup this service froze is refused: Podman still reports it as running, and a suspended universe is not a restored one. The command's exit code alone proves nothing. It then records ownership, archives an earlier `transferred` reservation for that universe, and writes `outbox/<authorization_id>/outcome.json`.
- `migration_restore_abort` (`authorization_id`, optional `reclaim_processes`) is the only way to end a claim that was not verified. It never touches a running or verified universe. For a held claim it removes only a non-running container created after that claim, labelled for the universe and owned by no verified operation, verifies its absence, then records `not_restored` and writes the outcome. An authorization this host never claimed is declined the same way, so that a destination which cannot restore lets the source end the authorization explicitly.

  `reclaim_processes` is an explicit boolean, false when absent, and it is the only way PodMesh ever ends a process it did not start. Without it, an abort that finds processes of the failed attempt still in the container's cgroups **refuses without effect** and reports them: removing the container then would unlink the files they are writing without freeing the space. With it, PodMesh ends only what it can prove belongs to the attempt — membership of that container's own `/machine.slice/libpod-<id>.scope` or `libpod-conmon-<id>.scope`, and a start time at or after the durable claim, both re-read from `/proc` immediately before each signal, which excludes a PID reused in between. A command line naming the container is a diagnostic fallback once those cgroups are gone; it never authorizes a signal. The abort then waits, bounded, for both cgroup directories to disappear, verifies the container's absence and measures the graph root before and after. A cgroup that does not disappear, or a process that survives, is an **incomplete** reclaim: the abort refuses, keeps the claim and preserves the evidence rather than claiming space it did not recover. Every PID, its cgroup, its start time, the decision and its result, the requester's `authorization_ref` and the field itself are recorded in the result and in `migrations/<claim operation>/reclaim-<operation>.json`.

  Who may ask for it is provenance, not proof: the request is meant for the root tandem, the operator with the agent acting under his direct control, and a maker forwarding a governor's row does not by itself carry that authority — that mapping is a later product decision. The root-only local socket remains the access boundary, as it is for every operation; PodMesh records the reference it was given and still refuses to signal anything it cannot prove, whatever the requester claims.

Ownership on the destination: a restored container keeps the source journal's creation label, which this journal does not know. `owned()` therefore accepts a verified `create`, `clone` **or** `migration_restore` whose recorded container ID is the observed one; a label alone stays insufficient, and a container this host recorded as transferred away never regains ownership through its original creation. Restoring with `--name` gives each hop a new container ID, which the outcome and the journal record.

Retries and interruption: a verified restore is never restored again; its replay is historical and adds `current_outcome`, a fresh re-hash of the written outcome. If the service is killed while the restore command runs, the command survives in its scope; a retry of the same operation is refused while that scope is still running and then reconciles by observation to one container and one outcome (`finalized_after_interruption: true`). A restore that fails after Podman started holds its claim, writes no outcome and preserves its diagnostics until an abort. While a claim is unresolved, `create`, `start`, `delete` and `clone` for that universe are refused, exactly as under a reservation.

What a restore attempt may consume: a restore of a damaged archive was measured on the laboratory writing its CRIU log at about 20 MB/s while `podman container restore` never returned, from processes living in the container's own cgroups rather than in the transient scope PodMesh created — stopping that scope does not stop them. Every restore, on a destination and locally, therefore runs under a bound. While the command runs, the free space of the Podman graph root is watched once a second, and the container the attempt created is identified from the cgroups that appear under `/machine.slice` (Podman itself does not answer during a restore). If the attempt consumes more than the space its own preflight required for it, or the graph root falls below one gibibyte, PodMesh **freezes that container's cgroup** — which stops the writing without ending anything, and is undone if the frozen cgroup turns out not to be the attempt's — and stops the transient scope. The restore then fails verification and holds its claim as usual, with the whole measurement (allowance, minimum free space, most consumed, samples) in the result. The frozen processes are left for an explicit `migration_restore_abort`, with or without `reclaim_processes`.

Scope and conmon lifetime: `systemd-run --scope` sets `INVOCATION_ID` for the command it runs, and Podman then leaves conmon inside that scope, which would stay active for as long as the restored universe runs. The variable is therefore removed inside the scope as well: conmon moves to its own `libpod-conmon-<id>.scope`, the transient restore scope ends with the Podman command, and stopping or deleting the universe through the API removes the conmon scope with it. `--keep` is what preserves the CRIU restore log the verification requires; the kept checkpoint files stay in the restored container's storage until the universe is removed or checkpointed again, and a later export from that container may also carry image files left by the previous restore.

## Experimental: garbage collection on proof (development tree, not packaged)

Age never justifies collection; proof does. The complete contract is [GARBAGE-COLLECTION.md](GARBAGE-COLLECTION.md):
the four kinds of act a collector must never confuse, the exclusions that make an unknown fact a blocker, the
terminal classes with the proofs each requires, the two modes, the record every run owes and the bounded
execution rules. This section describes what the development tree implements of it. There is **no timer**:
nothing here ever runs on its own, and no monitoring agent exists.

Both operations are host-wide — they are the only ones that name no universe — and both carry
`operation_id` and `authorization_ref` like every other operation, with the same replay contract.

- `garbage_collect_plan` (read-only; optional `max_candidates`, default 20, maximum 100, and optional
  `universe_uuids` to scope it to exactly the universes the caller is deciding about) enumerates a bounded
  set of reservations that still owe someone a decision and of unresolved restore claims, gives each a class
  or none, and records every proof fact it observed and every blocker it found, with the effect it would
  propose. It changes no reservation, claim, authorization or artifact, sends no signal, deletes nothing and
  makes two read-only Podman calls (one inventory and one batched inspection, and more only if that batch
  loses a race to a disappearing container); the only things it writes are the operation and attempt rows
  every operation writes, and its own immutable run record. That is exactly what the contract permits a dry
  run to do since its 2026-09-13 amendment: read-only Podman observation, and no durable write but its own
  audit trail. Settled reservations are counted, never
  examined, and a verified restore claim is never a candidate. A repeated operation ID returns the recorded
  plan as history with a fresh observation. Only the *number* of candidates is bounded: the count of settled
  reservations a host-wide plan reports, and the size of one candidate's proofs, are not.
- `garbage_collect_apply` (`plan_operation_id`, `candidates`, optional `max_effects` — default 1, maximum 10
  — `max_runtime_reclaims` — default 0, maximum 5 — `max_bytes` — default 4 GiB, maximum 1 TiB — and
  `reclaim_processes`) acts only on candidates a
  recorded plan of this host examined, classified the same way and found collectable. Immediately before
  each effect it repeats that candidate's whole classification from fresh facts and refuses on any mismatch;
  afterwards it verifies the result from outside, and an observation it cannot make is an unknown that fails
  that verification rather than a silence that passes it. It stops at the first mismatch: when nothing has
  been applied the request is refused and no run record is written, and once an effect exists the run
  reports what it did and where it stopped, never that nothing happened. A run is `verified` only when every
  effect it carries verified; otherwise its record says `completed_with_unverified_effects` and names the
  blockers. Each effect is recorded durably in the same transaction as the effect itself, so a retry of the
  same operation ID after an interruption **recovers** what was already committed instead of repeating it,
  and a replayed operation ID returns its historical record and repeats nothing. A plan enumerates
  reservations, unresolved restore claims and prepared recovery points.

Four classes are implemented: the three below, and class 5 **for recovery point archives only** (further
down). Class 4 (a failed local restore) is **not**, and checkpoint artifacts are not collected: the only
files this version ever removes are a recovery point's archive and manifest, after the conditions stated for
class 5, and the manifest survives in the journal.

- **Class 1, `terminal_reservation_all_authorizations_not_restored`.** A `checkpointed` reservation whose
  every authorization ended `not_restored` — the dead end lot M3 named, where no release and no abandonment
  are possible. The proofs: the recorded handoff and outcome of each authorization re-hashed against the
  values recorded when they were issued and completed; each outcome re-parsed and re-bound field by field
  (format, authorization, handoff hash, universe, source host, destination host, `not_restored`, no restored
  container); no authorization open for the universe; no unresolved restore claim; and a fresh observation
  showing the recorded container under the universe name, carrying its label, in `created`, `exited` or
  `stopped`, still `Checkpointed`, and not frozen.
- **Class 2, `terminal_reservation_container_absent`.** A reservation whose container is absent both under
  the universe name and under its recorded ID anywhere on the host, for which no authorization was ever
  issued and which has no unresolved restore claim. This is the abandonment shape, swept with its proofs.
- **Class 3, `failed_restore_claim`.** An unresolved restore claim. The plan proposes it and reports the
  container, the restore scope and the cgroup facts it rests on; the effect is `migration_restore_abort`
  itself, called with the run's operation ID and the request's explicit `reclaim_processes`, whose refusals
  are the collector's refusals. The collector enumerates, signals and waits for no process of its own: a
  runtime reclaim needs `reclaim_processes: true` **and** a `max_runtime_reclaims` of at least 1.

The effect of a collected reservation is a terminal `collected` state and a **tombstone**. `collected`
blocks no generic operation, exactly as `released` does not: the stopped, checkpointed source of a class 1
collection can then be started, stopped, deleted or resumed by `migration_restore_local`, which is a
separate explicit operation with its own result — a collection starts, stops and removes nothing. A new
`migration_checkpoint` supersedes a collected reservation exactly as it supersedes a released one, archiving
it with its history, so a collected universe can be checkpointed, authorized and migrated again. The
tombstone is what stays: `create` with that universe UUID, and a `clone` into it, are refused for good, even
after a verified local restore or a new migration has brought the universe back, and a container any
collection proved absent never regains ownership through its original creation. Reusing a collected identity
needs a verified handoff restore or an explicit replacement procedure. A tombstone is never removed and
never rewritten: it keeps the proof of the first collection, while `migration_collection_history` records
every collection of that identity, and `migration_status` reports both beside the reservation.

Every run is recorded in `garbage_collection_runs` with its operation ID, requester reference, collector and
policy version, mode, bounds, start and end, every candidate with its class, proofs and blockers, and, for
an apply, what was attempted, what each effect achieved and the verification that followed it, plus the
runtime reclaims it reserved, the ones it performed and the processes it signalled. Each individual effect
is also recorded in `garbage_collection_effects` inside the effect's own transaction, which is what lets an
interrupted run recover. A tombstone carries a copy of the proofs its collection rested on.

- **Class 5, `recovery_point_archive_after_retention`.** The archive and manifest a `recovery_point_prepare`
  left in this host's outbox. A point is a candidate only when **all** of these hold, each recorded as a proof
  or a blocker: the point is `prepared` and its manifest still hashes to what its record binds; a retention
  is **declared** for the universe (`collection_retention_declare`, below — without one there is no elapsed
  retention to prove, and the plan says so rather than assuming one); the point is outside the newest
  `keep_latest` generations and older than `minimum_age_seconds`; no hold applies and the hold state could be
  read; the outbox path recomputed from the identifier agrees with the record; the archive is a regular file
  of the recorded size — and, immediately before the effect, it still hashes to the recorded digest, so a
  tampered archive is kept as evidence and never collected. The effect writes a **retained manifest**
  (`recovery_point_retained`: the manifest's full content, both digests, sizes, the preparing and collecting
  operations, the terminal state and the retention it was collected under) and marks the point `collected`
  in one transaction, and only then removes the archive, the manifest file and the directory, verified from
  outside. A run interrupted between the two finishes the removal on retry rather than repeating anything;
  a replay of the collection repeats nothing; and a replay of the point's own `recovery_point_prepare`
  serves the retained manifest with `collected: true`. `max_bytes` (default 4 GiB, at most 1 TiB) bounds
  what one run may remove and is reserved before each removal. Inbox copies on a standby are the transport
  controller's, which wrote them, and are not collected. Plans share their candidate bound three ways so
  that archives filling a disk cannot be hidden by dead reservations.

**Both hold scopes are implemented**, as the contract amended on 2026-09-13 defines them, and interpreted in
one place (`retention::hold_blocks`): an `investigation_hold` blocks every apply effect of every class; an
`evidence_hold` blocks artifact deletion (class 5) and runtime reclaim (class 3 with `reclaim_processes`)
and not a history-preserving terminal reservation transition. A hold that cannot be read is a hold: the
effect it would govern is blocked and the plan says why. Holds never expire on their own; a hold ends when
someone releases it under their own operation, and the release is kept beside it.

- `collection_retention_declare` (`keep_latest` 1–1000, `minimum_age_seconds` 0 to ten years) declares, per
  universe, how many newest recovery points are kept whatever their age and how old a point must be before
  it is a candidate. At least one is always kept. The `authorization_ref` is kept as `declared_by`.
- `collection_hold_declare` (`scope`: `evidence_hold` or `investigation_hold`, `reason`) places a hold; its
  identity is the declaring operation, so a replay is the same hold. `collection_hold_release` (`hold_id`)
  ends it and records who did. `collection_status` reports the retention, the holds in force, the released
  ones and the retained manifests of a universe.

The class 3 reclaim path under an evidence hold is not reachable from a single host; `hold_blocks` is
unit-tested for it and the apply-time guard says so beside the code.

Who may apply is provenance, not proof, exactly as for a reclaim: the request records its
`authorization_ref`, the root-only socket remains the access boundary, and the collector still refuses
anything it cannot prove. A plan authorizes nothing.

## Experimental: activation leases and recovery points (development tree, not packaged)

The design is [UNIVERSE-HIGH-AVAILABILITY.md](UNIVERSE-HIGH-AVAILABILITY.md); the recovery point's format is
[BACKUP-SERVER.md](BACKUP-SERVER.md). Every operation here carries `operation_id`, `universe_uuid` and
`authorization_ref` with the same replay contract as the rest — through the same journal, so a repeated
request under its ID is flat history marked `replayed` and `historical`, a different request under that ID is
refused, and an interrupted one is re-evaluated — except `activation_fence`, which is host-wide and names no
universe, and the read-only `*_status` operations, which carry no operation ID. There is **no timer** and no failure detector: nothing here runs on its own, and the
timeliness of renewals and fences is the caller's obligation.

**Activation.** A universe under a policy may be started, cloned or restored only by a host holding its live
lease. `stop` is never gated.

- `activation_require` (`lease_seconds` 5–3600, `takeover_margin_seconds` ≥ 5, optional `desired_standbys`
  0–16, `eligible_hosts` and `authority_id`) declares the policy. Absent standbys means none. A target no
  placement among the eligible hosts can satisfy is refused at declaration. The `authorization_ref` is kept
  verbatim as `allocation_decided_by`: the allowance is the operator's judgement and is never computed here.
  An `authority_id` names the external gate whose **epochs** bind activation (below); absent means leases alone.
  With it, an optional `authority_key` (the gate's Ed25519 public key, 32 bytes as lowercase hex, refused unless it
  is a valid point) makes every takeover document for the resource require that key's signature (below); a key
  without an authority is refused. `activation_status` reports `authority_key` and `takeover_proof_verification`.
- `activation_acquire` takes the lease for this host. It is idempotent while this host's lease is live, retakes
  this host's own lapsed lease with a new generation, and takes over another host's only once that lease has
  lapsed by **at least the takeover margin** — before that it is refused and says when it may be taken. Under an
  authority it also requires a `permit` (below) whose epoch is newer than the previous holder's.
- `activation_renew` extends this host's live lease; a lapsed lease is **not** renewable and must be
  re-acquired, so an entitlement that ended is never silently extended; a superseded one is not renewable either.
- `activation_supersede` (`permit`) delivers a newer grant, bound to whoever it was bound to, so that this host
  learns it has been overtaken: the screen advances, and this host's lease, live or not, no longer entitles it —
  the gate, the renewal and the fence all read that. A permit at or below the highest epoch seen is refused, so
  the screen only ever moves forward.
- `activation_release` surrenders it. `migration_complete_transfer` releases it under its own history event,
  `released_by_handoff`.
- `activation_status` reports the policy, the lease, its history, the replication intent, this host's
  resources (memory available, CPU count, one-minute load, state-directory space — facts, never a decision)
  and a `scope` sentence stating what the lease proves.
- `activation_fence` (`timeout_seconds`; no `universe_uuid`) stops every universe under a policy that this host
  holds no live lease for, and reports which it left alone and why. It must be called at least as often as the
  shortest lease, or a lapsed lease leaves a universe running. It also withdraws every exclusive route (below)
  published under a resource this host no longer holds a live, unsuperseded lease for, verified from the
  kernel, and reports them as `routes_withdrawn`; a withdrawal that does not take is reported, never claimed.
  It reconciles the network ledger first (`network_reconciliation`).
- `activation_fence_preview` (read-only, not journaled; no fields) says what a fence would act on now:
  `pending` (universes running without entitlement, exclusive routes without entitlement),
  `incomplete_network_effects`, and `nothing_to_fence`. A timer asks it first and journals a fence only when
  there is something to fence, so an idle host's journal does not grow with empty ticks.

**Epochs, from the fencing laboratory.** `experiments/manager-fencing` in the web tree models exclusion as an
epoch issued by one external gate, rotated only by an explicit trusted action, with each maker keeping a durable
screen that refuses epochs it has seen superseded. A policy with an `authority_id` puts PodMesh in the maker's
role. A `permit` is the laboratory's exact form — `authority_id`, `resource`, `epoch`, `replica_id`,
`instance_id`, `grant_id`, no other field; identifiers `[A-Za-z0-9][A-Za-z0-9_.:-]{0,95}`; epoch 1 to 2³¹−1 —
and is bound to the universe (`resource`), this host (`replica_id`) and **this boot** (`instance_id` is the
kernel's `boot_id`, so a rebooted host must be authorised again). The screen, `highest_epoch_seen`, is reported
by `activation_status` with `superseded`. The gate's fourth refusal is "superseded by epoch N".

**The takeover document, signed.** The authority's account of a rotation (`takeover_proof`, consumed by
`publisher_start`) comes in two kinds. Under a policy that names an `authority_key`, only
`podmesh-takeover-proof/ed25519` is accepted: `signer` must be that key, and `signature` (64 bytes, hex) must
verify under it over the document's canonical form — the document without `signature`, every object's keys
sorted, compact JSON, no floating-point number — **before** any field of the document is read, so that an
altered document, one signed by another key, or an unsigned one is refused as such. Under a policy without a
key, only `podmesh-takeover-proof/lab-unsigned` is accepted, on its binding alone, and the answer says so
(`signed: false`). PodMesh holds no key of its own; the tool's gate generates the authority's signing key at
first use (`PODMESH_HA_KEYS`, one root-only file per authority) and names its public half in every policy it
declares. A permit is still unsigned provenance (below): what the signature closes is the origin of the
takeover document, not of the permit.

**What a lease proves.** This host's own restraint: it will not start what it holds no lease for. It does
**not** prove mutual exclusion — the lease lives in this host's journal, a host that never asks is not
restrained by it, and the takeover margin is measured against this journal's copy. Every status answer and every
promotion says so. **What a permit proves:** provenance from a root-only channel, and nothing more — PodMesh never
contacts the gate and holds no key, so it cannot verify a permit's origin. The asymmetry is stated in every
answer's `permit_verification`: a forged higher epoch can stop a universe here, never start a second one. Gate
uniqueness and compare-and-swap rotation are the laboratory's, not PodMesh's.

**Recovery points.** A stopped universe becomes an immutable, digested point; a point becomes a quarantined
copy; a quarantined copy becomes the universe itself, under the lease.

- `recovery_point_prepare` exports a **stopped** universe to `outbox/<recovery_point_uuid>/` with a canonical
  manifest. A running universe is refused rather than stopped; a stop that escalated to SIGKILL (exit code 137)
  has no consistency class and is refused rather than downgraded. The manifest is **unsigned** and says so:
  `signed: false`, `state: "prepared"`, a format string ending in `unsigned-unencrypted`. No signing dependency
  exists in this build, and adding one is the operator's decision.
- `recovery_point_status` lists a universe's points by generation.
- `recovery_point_restore` (`recovery_point_uuid`) creates a **quarantined, new-identity** universe from a point in
  `inbox/<recovery_point_uuid>/`: no network, not started, under a `universe_uuid` that must differ from the
  source's — even when the source is unknown here. The archive is checked against the manifest's size and digest,
  the manifest against its canonical form and pinned format, and a manifest that **claims a signature is
  refused**: this build cannot verify one, and a signature nobody can check is not a signature. The manifest's
  origin is not verified, and the answer's `manifest_verification` says so. The container is created through
  the ordinary `create` under the derived operation ID `<operation_id>-create`, so ownership needs no new rule.
  The image it imports is tagged `localhost/podmesh-restore:<recovery_point_uuid>` and is removed by `delete` of
  its last user — the quarantined copy or the universe promoted from it — never while a container still uses
  it and never if it carries a name outside that repository; `delete` reports `restore_images_removed` and
  `restore_images_retained` with the reason, beside the clone snapshots' own fields.
- `recovery_point_promote` (`restored_universe_uuid`, `network_profile` required as for `create`, optional
  `network_address` under the managed profile) creates the universe named by `universe_uuid` from exactly
  the image and command of that quarantined copy — read from the copy's own verified create — under the
  profile named, and not started. The restore's answer reports `source_network` (profile, address and
  network UUID from the labels the manifest recorded), so a universe put back can be promoted at the address
  allocated to its UUID; nothing allocates on the caller's behalf. The answer's `network` names the profile
  and requested address the universe was created with. Refused, in this order: no such copy here; a copy of a different universe; a copy promoted
  into itself; no activation policy for the universe on this host; then the lease gate's three reasons. The
  quarantined copy is left in place. The answer carries the lease generation and the `scope` sentence above.
- `recovery_point_prepare` with `capture: "live"` takes a **running** universe instead: the qualified runtime
  dumps it with its memory, Podman exports the writable layer while the dump holds its processes stopped, and
  the universe resumes in place from the kept images, which are then removed. The point holds
  `checkpoint.tar.zst`, its class is `memory-coherent`, and the answer reports `dump_seconds`,
  `resume_seconds` and `interruption_seconds`. It takes the migration checks (network none, no mounts, no TTY,
  bounded memory) and, under a policy, the lease gate, because the resume is a start. The attempt is recorded
  before the dump; a retry of a capture the service did not see to the end records no point, resumes the
  universe from its kept images if the dump left it stopped, and reports what it found.
- `recovery_point_stage` (`recovery_point_uuid`) records a live point from `inbox/` on a standby, creating no
  container: a second instance of a running universe must not exist even stopped. It refuses, with every
  blocker listed, what a promotion would discover too late: the image absent or its recorded name not
  resolving here to the same ID (Podman's import would pull), a runtime binary, runtime git ID or kernel
  release different from the source's, an archive whose configuration does not name the image and
  universe, missing CRIU entries, too little space.
- `recovery_point_discard` (`recovery_point_uuid`) removes a staged point's files and keeps its record; a
  promoted point is refused.
- `recovery_point_promote` with `recovery_point_uuid` instead of `restored_universe_uuid` restores a staged
  live point **running**, with its memory, network none; there is no separate start. Refused, in this order:
  no such staged point here; staged for another universe; discarded; already promoted; a profile other than
  isolated, or secrets or an address; a migration reservation or unresolved restore claim; a collected
  universe's tombstone; no activation policy; the lease gate's three reasons; a container named or labelled
  for the universe on this host. A durable attempt row precedes the restore; a replay of an attempt that
  reached it decides by observation (a container for the universe restored since the attempt began) and
  never restores twice. The container is owned by the verified promotion, for `stop`, `delete` and the
  collector.

How the two files reach the inbox is the transport controller's, as for migrations: PodMesh reads
`inbox/` and never writes it. The two-host suite's controller carries an outbox to an inbox over SSH.

## Experimental: the universe network (development tree, not packaged)

The contract is `UNIVERSE-NETWORK-CONTRACT.md`; the operations are journaled like every other and verified from
`podman network inspect`, `ip route` and `nft`, never from the tables alone. Every kernel or Podman mutation is
recorded in an effects ledger before it is made; a failure after an effect compensates what was made and says
so; reconciliation runs at daemon startup, before every network mutation (`reconciliation_before` in the
answer) and at every fence, undoing whatever was left half-made and refusing every mutation while anything
remains (`network_status`: `effects`, `incomplete_effects`).

- `network_declare` (host-wide; `network_uuid`, `prefix`, `pool`, optional `peer_pools` `[{pool, via}]`, optional
  `nat_exemption`: `null-snat` by default, `notrack`, or `none`): the bridge, the peer routes, and the nftables
  table `inet podmesh-managed` that keeps Podman's source NAT off traffic inside the prefix — by default a null
  source NAT of the local pool's traffic to the prefix that keeps connection tracking; `notrack` removes it, and
  a stateful firewall then blocks that traffic (measured) — each verified from outside; `network_undeclare`
  (host-wide; `network_uuid`) removes all three and verifies their absence; `network_status` (read-only)
  reports them, the exemption as `nat_exemption` with its backend and rules.
- `create` with `network_profile` `managed` allocates the next free address of the host's pool to the universe
  UUID, or the one named by `network_address` inside that pool; `delete` releases it.
- `network_route_publish` (host-wide; `universe_uuid`, `ip`, `via`, optional `exclusive_resource`) publishes the
  `/32` of an address placed elsewhere; refused while any route for that address is effective here or while
  the universe is allocated here. With `exclusive_resource` the route is the **exclusive effect of a role**
  — a logical manager's service address, for instance — and is published only by the host holding a live,
  unsuperseded activation lease on that resource under the epoch gate: refused, in this order, when the
  resource is under no activation policy here, then for the lease gate's four reasons (none held, held
  elsewhere, expired, superseded). The route records the resource, and `activation_fence` withdraws it once
  the lease is gone. An exclusive route must point at a running universe of this host, which then carries
  the address as an alias inside its network namespace (added before the route, verified from inside,
  withdrawn with the route; `alias_universe_uuid` and `alias_effective` in `network_status`); refused when
  nothing runs at `via` here. `network_route_withdraw` (`universe_uuid`) removes the routes of a universe,
  and the alias with them.

## Experimental: secrets a universe is given at creation (development tree, not packaged)

A secret's bytes never enter an image layer, this journal, or the API line. The operator (or the agent, over
root SSH) places the file under `inbox/secrets/<source>` of the state directory, root-owned with no group or
other permission, and asks `secret_declare` (`name`, `source`): the daemon records the name and the intended
digest first (`declaring`), hands the bytes to Podman's secret store, reads the store's content back and
requires it to hash to the intent, records `effective`, and removes the inbox copy. Names are immutable: the
same content under the same name is idempotent, another content is refused and needs another name (then the
universe is switched to it). `secret_remove` records `removing` before clearing the store. Reconciliation
(at startup, before every network mutation, at every fence) finishes a `declaring` secret whose store content
hashes to the intent, removes one whose content differs or is absent, and finishes a `removing` one. `create` takes `secrets`
`[{name, target}]`: each must be declared here and present in Podman's store; it is mounted at the target,
root-only (0600), and the container is labelled with names and targets only. `recovery_point_restore` reports
the source's `source_secrets` by name; `recovery_point_promote` takes `secrets` to attach them again, once
declared on the promoting host. `secret_remove` (`name`) refuses while any container carries the name, running
or stopped. `secret_status` (read-only) lists names, digests and presence in the store, never content. The
durable copy of a secret is the operator's, outside PodMesh; Podman's store at rest is root-only files on the
host, the laboratory's accepted boundary.

## Experimental: the publishing connector that follows the governor (development tree, not packaged)

The contract is `MANAGER-PUBLISHER-CONTRACT.md`. `publisher_declare` (`resource`, `hostname`, `tunnel_uuid`,
`credential`, optional `origin_port`) records the connector by reference; `publisher_start` (`resource`,
`takeover_proof`, optional `previous`) is refused unless the resource's lease is live and unsuperseded here,
the exclusive route and alias are effective, the credential is in Podman's store and the authority's takeover
proof is of the kind the policy requires, its signature verified when the policy names the authority's key, and
binds this transition (resource, epochs, holders, expiry, and its method: first, same holder, fence
receipt, or a lease barrier already reached on this clock); then records its transition `starting`, writes
the governor mark inside the carrier universe, requires the origin to answer ready with the expected logical
manager, replica and epoch, runs `cloudflared` as a transient unit from a root-only runtime copy of the
credential, waits for its registration with Cloudflare, and records `effective`; the answer carries
`published` and `connector_id`; `publisher_stop` stops it and removes the mark; `activation_fence` does the same first for every
resource this host no longer holds (`publishers_withdrawn`); `publisher_observed` records an external request;
`publisher_status` (read-only) reports the unit, the connector's identity, the lease, the origin's readiness and
`publisher_eligible` with reasons. Every step is recorded in the network effects ledger before it is made.

## Experimental: the agent's door to a manager universe (development tree, not packaged)

A manager universe runs the frozen manager resident behind one Unix socket at a contract path inside the
universe (`/run/podmesh-manager/control.sock`), reachable by nothing outside it. PodMesh offers the one door:
two typed operations over the same root-only API, with `authorization_ref` as provenance, carried by a copy of
the daemon entered into the universe's PID namespace (the resident checks the peer's credentials, and a peer
whose PID is not visible from the universe is refused whatever its UID). The PID read from Podman is bound to
the container it was read from — the process must sit in that container's cgroup, and its start time is
recorded — and checked again right before the relay is spawned and after it returns: a universe restarted or
replaced in between is refused ("changed between its inspection and the relay"), and an answer that came
back after such a change is not trusted. The agent never sees the socket, and PodMesh adds nothing to the
resident's protocol.

- `manager_status` (read-only): the resident's bounded live diagnostic (replica identity, peer counters), never
  its facts, which are read from the store. Refused when the universe is not here, not running, or carries no
  control socket at the contract path (it is not a manager universe).
- `manager_observe` (`scope`, `subject`, `value`; journaled): appends one observation in a scope this replica
  owns, with the PodMesh operation ID as the resident's, so a re-evaluation after a crash between the append
  and the record is a replay for the resident too. Bounds checked before any connection (safe tokens of at most
  128 bytes, a hierarchical scope, a value of at most 4096 bytes; the API line limit of 4 KiB applies first);
  the resident's own refusal (a scope it does not own, a writer it does not accept) is returned as the refusal.
  Not an exclusive effect and not gated by the activation lease: every replica appends in its own scopes and
  replication carries them.

## Facts for a watching agent

The intended first consumer of these read-only facts is a watching agent that observes and reports: it has
no hand on the host. `migration_status` therefore carries a `watch` object beside the detailed rows, so
that one call answers, without running anything: whether a reservation still blocks the universe and
whether it is still awaiting a decision, with the time it last changed; which restore claims are
unresolved and since when; which transfer authorizations are still open; whether a failed restore left
runtime processes, with the cgroup facts each conclusion rests on; and how much room is left on the
Podman graph root against what an unresolved attempt was allowed to consume.

Every fact carries the time it was observed, and what cannot be established is reported as unknown with
its reason — never as a healthy zero. A surviving-process count is authoritative only when it comes from
cgroup residency (`source: cgroup_residency`, `authorizes_reclaim: true`); once those cgroups are gone the
command-line fallback is a hint about what may remain, and a claim that never recorded a container reports
`known: false` rather than none. A reader that finds no `watch` object, or one older than it expects, is
looking at a stale answer and should say so.

A reclaim remains an explicit request carrying the root tandem's provenance. A watcher that concludes one
is needed is making a proposal, not an authorization: nothing in this version verifies who asked, the
root-only socket is the whole access boundary, and PodMesh still refuses to signal anything it cannot
prove belongs to the failed attempt it claimed. `migration_status` is per universe: enumerating the
universes to watch is the reader's job, from `inventory` and its own records. The one host-wide read-only
listing that does exist is `garbage_collect_plan`, which enumerates the reservations that still owe a
decision and the unresolved restore claims, with their proofs and blockers; it is a plan, and a plan
authorizes nothing.

## Authority and scope

`authorization_ref` records provenance; it is not a verified authorization token. The root-only local endpoint is the present access boundary. Remote authentication, tenant policy and ShaperOS integration remain separate work. Direct Podman commands may be used for independent verification and test fixtures, but do not count as a successful PodMesh operation when its API is absent.
