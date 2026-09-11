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

The UUID and image above are placeholders, not executable examples. The CLI sets the operation field from its first argument. The initial creation operation produces a stopped container with networking disabled; it does not start the application. Image pulling is not implicit.

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

## Timeouts, interruption and cancellation

There is no cancellation operation. The service handles one request at a time; a client disconnect or client timeout does not cancel the operation, which continues to completion. Each Podman call is bounded (30 s; commits 300 s; stop `timeout_seconds` + 30 s); a call exceeding its bound is terminated and the operation fails with the re-observed state, not an assumed one. If the service itself is killed, the operation remains `pending`. Every attempt is recorded in the `operation_attempts` table.

Retrying a pending or failed operation re-evaluates the observed state. To avoid repeating an effect, a retried **start** does not start again if the container has started since the first attempt began, and a retried **stop** is refused if the container may have been started after the first attempt began (it would stop a newer run). Timestamps have one-second resolution; the comparison is conservative. Killing the service during a stop wait interrupts `podman stop` before any escalation: Podman then records the container as `stopping` while the application is still alive. PodMesh reports such a container as `running: true` with `state: stopping` and a note, refuses to start it, and accepts a stop (a retry of the interrupted operation or a new one) to complete it under that request's declared timeout behavior.

Started containers keep running when the PodMesh service stops, restarts, or is upgraded or removed: Podman's container monitor (conmon) runs in its own `libpod-conmon` scope, not in the service cgroup.

## Results and retries

Check both CLI exit status and JSON `ok`. Keep the same operation ID and byte-equivalent semantic request when retrying. Reusing an operation ID for a different request fails. A verified operation is never executed again: the response is `replayed: true, historical: true`, with `original_result` as persisted when it was verified, `verified_at` when recorded, and `current`, a fresh Podman observation of the universe container, plus `current_matches_recorded_container`. The historical result may contradict current state; use `current`, inventory or independent inspection for present facts.

A dropped connection is an uncertain outcome, not proof that no action occurred. Preserve the request and inspect or retry it according to the operation contract. Do not generate a new operation ID blindly after a timeout.

## Experimental: source-side migration preparation (development tree, not packaged)

These operations prepare and checkpoint a migration **source**. Transfer of authority to another host and the destination side are the separate operations documented in the next section; no release, abandonment or local restore of a reservation exists (lot M3). See MIGRATION-INTEGRATION.md and MIGRATION-PROTOCOL.md for the protocol and its remaining gaps.

Requests carry the full binding: `operation_id`, `universe_uuid`, `authorization_ref`, `container_id` (full 64-character ID), `image` (full `sha256:` image ID), `source_host_uuid` (must be this host) and `destination_host_uuid` (recorded, not contacted; must differ from the source).

- `migration_preflight` reports `compatible` and `blockers` with fresh facts and has no effect: no reservation, signal, suspension or artifact. Like every operation, repeating the same operation ID returns the historical report; a fresh assessment uses a new operation ID. Blockers include: not owned by this host's journal, identity mismatch, not running, network mode other than `none`, volumes or bind mounts, privileged or TTY containers, a frozen cgroup, more than 64 processes, container memory above 1 GiB or unreadable (a dump still running at the 300 s bound is killed, which destroys the application), any process that is not musl-based (glibc registers rseq, which the qualified CRIU 3.15 predates; static binaries cannot be identified), a Podman graph root other than the default store, insufficient space in the state directory or the graph root, an existing reservation, and a private runtime whose binary, wrapper or shim differs from the pinned SHA-256 or whose `criu check` fails.
- `migration_checkpoint` repeats every check immediately before acting and refuses before any suspension if one fails. It then persists a reservation, creates `migrations/<operation_id>/` under the service state directory (0700, files 0600), and runs `podman container checkpoint --export --compress=zstd --keep --file-locks --print-stats` on the reserved container ID (not the universe name) with `PATH` beginning at the packaged private shim `/opt/podmesh-vzcriu-kit/bin`, inside its own transient systemd scope `podmesh-checkpoint-<operation_id>.scope`, bounded to 300 s. The distribution CRIU is never used. The result is verified only if Podman reports the same container `Checkpointed` and stopped, the archive lists the expected entries, and the copied CRIU log shows a successful dump by `v3.15.5.3`. The directory keeps `checkpoint.tar.zst`, `manifest.json`, `dump.log`, `preflight.json`, per-attempt stdout/stderr and failure records; the result carries the archive and manifest SHA-256.
- `migration_status` (read-only, `universe_uuid` only) returns the reservation, a fresh observation, a re-hash of the preserved archive and manifest, and the release preconditions it can observe, with `release.permitted: false`.

A reservation blocks `create`, `start`, `delete` and `clone` for that universe (as target or source); `stop` remains available: it cannot run, replace or remove the source, but stopping a source whose checkpoint failed ends its process and memory state. Earlier package versions running on the same journal after a rollback do not enforce reservations.

Retries: a verified checkpoint is never captured again; its replay is historical and adds `current_artifacts` with a fresh re-hash. A pending or failed checkpoint is re-evaluated: if its scope has not finished (including activating or deactivating) or its state cannot be queried, the retry is refused; if Podman shows the reserved container checkpointed after the reservation, the existing archive is finalized without capture (`finalized_after_interruption: true`); if the same process is still running and was never checkpointed, it is checkpointed again; anything else is refused with the observed state, and nothing is restarted. Killing CRIU itself mid-dump was observed to destroy the application without an archive; the separate scope protects against a service crash, not against CRIU or host failure.

## Experimental: serial migration between two hosts (development tree, not packaged)

The operations above prepare and checkpoint a universe. The ones below move the authority over that universe to one named destination host, and back. The protocol and the deviations this implementation recorded are in MIGRATION-PROTOCOL.md. Nothing here is packaged or qualified for ordinary workloads: a reservation is not fencing, and direct administration bypasses it.

Documents travel through fixed directories under the service state directory: `outbox/<authorization_id>/` is written only by the service, `inbox/<authorization_id>/` by the transport controller as root. Requests carry an `authorization_id` and never a path, so the 4 KiB request limit never has to carry a document. A document is not a credential: every value in it is checked against this host's journal and against bytes the service hashes itself. Root on either host can forge documents and journals; that threat is out of scope, as it already is for every local operation.

### Source

- `migration_authorize_transfer` (`checkpoint_operation_id`, `destination_host_uuid`) moves a `checkpointed` reservation to `transfer_authorized`. It re-hashes the preserved archive and manifest, observes that the reserved container is still checkpointed and not running, and checks the manifest bindings. The authorization row, with a service-issued `authorization_id` and the exact handoff bytes, is written **before** any artifact leaves the host; the outbox then receives `checkpoint.tar.zst`, `manifest.json` and `handoff.json` (`podmesh-transfer-handoff/1`: universe, source container, image, both host UUIDs, checkpoint operation, archive bytes and SHA-256, manifest SHA-256, the runtime git ID and binary SHA-256 and the kernel release recorded when the dump was finalized, issue time and the requester's `authorization_ref`). A replay returns the same handoff with a fresh re-hash of the outbox; a second authorization of the same reservation is refused.
- `migration_complete_transfer` (`authorization_id`) reads `inbox/<authorization_id>/outcome.json` and applies it: `restored` makes the reservation `transferred`; `not_restored` returns it to `checkpointed` and records the authorization as ended. The state machine then allows a new authorization for the same recorded destination; that retry is not exercised by the current tests. The outcome must be a `podmesh-transfer-outcome/1` document bound to this authorization's handoff SHA-256, this universe, this source host and the recorded destination. Any mismatch is refused without a state change, as is a source container that is running or has been replaced.
- `migration_retire_source` (`authorization_id`) removes, from a `transferred` reservation, only the stopped container that is still the reserved one and still checkpointed, together with the checkpoint files Podman kept for it. The reservation stays `transferred` and keeps refusing generic operations and `create` for that universe UUID; the evidence directory is kept. An already absent container is a verified no-op.

### Destination

- `migration_destination_preflight` (`authorization_id`) is read-only and reports `compatible` and `blockers`: the inbox handoff names this host and another source; no container `podmesh-<uuid>`, no container carrying the universe label, no active reservation (a local `transferred` history does not block a return trip) and no other unresolved restore claim; the image is present locally (PodMesh never pulls); the runtime binary hash, the runtime git ID and the kernel release equal the handoff's; the archive and manifest hash to the handoff and agree with it field by field; the archive's own `config.dump` names the handoff's source container, image and universe label, and the archive lists the expected checkpoint entries; both the state directory and the Podman graph root have room. Repeating the operation ID returns the historical report.
- `migration_restore` (`authorization_id`) repeats that assessment, copies the archive into its own operation directory and re-hashes the copy, persists a restore claim, then runs `podman container restore --import --name podmesh-<uuid> --keep --file-locks --print-stats` in its own transient scope with the private runtime first on `PATH`. It is verified only when Podman shows that container running and `Restored`, created and restored after the claim, carrying the handoff's universe label, from the handoff image, network-disabled and mount-free, **and** the preserved CRIU restore log shows a successful restore by the qualified runtime. The command's exit code alone proves nothing. It then records ownership, archives an earlier `transferred` reservation for that universe, and writes `outbox/<authorization_id>/outcome.json`.
- `migration_restore_abort` (`authorization_id`) is the only way to end a claim that was not verified. It never touches a running or verified universe. For a held claim it removes only a non-running container created after that claim, labelled for the universe and owned by no verified operation, verifies its absence, then records `not_restored` and writes the outcome. An authorization this host never claimed is declined the same way, so that a destination which cannot restore lets the source end the authorization explicitly.

Ownership on the destination: a restored container keeps the source journal's creation label, which this journal does not know. `owned()` therefore accepts a verified `create`, `clone` **or** `migration_restore` whose recorded container ID is the observed one; a label alone stays insufficient, and a container this host recorded as transferred away never regains ownership through its original creation. Restoring with `--name` gives each hop a new container ID, which the outcome and the journal record.

Retries and interruption: a verified restore is never restored again; its replay is historical and adds `current_outcome`, a fresh re-hash of the written outcome. If the service is killed while the restore command runs, the command survives in its scope; a retry of the same operation is refused while that scope is still running and then reconciles by observation to one container and one outcome (`finalized_after_interruption: true`). A restore that fails after Podman started holds its claim, writes no outcome and preserves its diagnostics until an abort. While a claim is unresolved, `create`, `start`, `delete` and `clone` for that universe are refused, exactly as under a reservation.

Scope and conmon lifetime: `systemd-run --scope` sets `INVOCATION_ID` for the command it runs, and Podman then leaves conmon inside that scope, which would stay active for as long as the restored universe runs. The variable is therefore removed inside the scope as well: conmon moves to its own `libpod-conmon-<id>.scope`, the transient restore scope ends with the Podman command, and stopping or deleting the universe through the API removes the conmon scope with it. `--keep` is what preserves the CRIU restore log the verification requires; the kept checkpoint files stay in the restored container's storage until the universe is removed or checkpointed again, and a later export from that container may also carry image files left by the previous restore.

## Authority and scope

`authorization_ref` records provenance; it is not a verified authorization token. The root-only local endpoint is the present access boundary. Remote authentication, tenant policy and ShaperOS integration remain separate work. Direct Podman commands may be used for independent verification and test fixtures, but do not count as a successful PodMesh operation when its API is absent.
