# Planned manual canonical NODE logical restore — unqualified source candidate

This root administrative helper exposes finite `export`, `restore`, and
`seal-source` commands. It uses the canonical NODE image, its actual store profile and private
MariaDB, and the external real host provider. It changes no NODE API, daemon,
MANAGER interface or private-unit delivery controller. The seal-capable canonical
entrypoint adds an early local startup refusal. The old
`delivery/instance.py` controls a different private-unit layout and cannot restore
this canonical single-container layout; it is deliberately not reused as an engine.

The helper is source only. Its new command, file metadata handling,
fault tests and native workflow require qualification before release. A previous
manual procedure passing is not qualification of these new command bytes.

## Supported state and authority

- The current canonical 40-table schema, all base tables InnoDB, at most1,000 rows
  per table. Output/file buffers use the explicit operator byte limit below.
  Views, routines, events and triggers
  must be absent; their presence refuses rather than discarding them silently.
- Exactly four empty native bootstrap directories under `sav/app`: `inbox`,
  `outbox`, `migrations`, and `podman-tmp`. The manifest records type, UID, GID,
  ordinary mode and emptiness. Fresh targets create their own directories; their
  metadata must match before import and through subsequent readbacks/recreation.
  Nonempty directories, symlinks and extra entries remain outside this scope.
  Bundles without this explicit directory map are refused; no old bundle is rewritten.
- A complete explicit list of at most16 flat regular files directly under
  `sav/app`. Each must be single-linked. Actual UID, GID and ordinary permission
  mode are recorded and restored, including a mode0644 marker. Special mode bits
  are outside this contract. Total file contents use the operator's byte limit.
  No directories, symlinks, arbitrary attributes, ACLs, xattrs,
  undeclared files, nested volumes or whole-filesystem restoration are supported.
- Export briefly SIGSTOPs the unique canonical UID1102 APP, retains its PID and
  starttime, captures SQL/schema/row/file inventories under the operator's
  already-established idle checkpoint contract below, and resumes only
  that APP in `finally`. MariaDB remains running. No source stop, fencing,
  promotion, cutover, SSH connection or key copying is performed.
- Restore accepts only a trusted completed root-private bundle. It creates two
  absent owned roots and an absent container name itself. Its fresh canonical
  birth generates new application and root DB credentials. Only that fresh
  target's application database is dropped, checked absent and imported; system
  tables and credential files are never imported.
- After exact import comparison, the target stops cleanly. On a different actual
  physical host, the explicit `--adopt-restored-host-identity` option authorizes
  the existing entrypoint's root completion request and transactional adoption.
  Logical NODE UUID is retained. Only machine binding and one audit row may
  differ. A subsequent ordinary recreation must preserve all post-adoption state.
  The command finishes with its owned target stopped cleanly and retained.

The operator supplies an exclusive local maintenance window: no competing root
administration, schema changes, SQL writers or lifecycle changes. The transfer
lock coordinates helper exports only; it is not a distributed lock or fencing.
The provider must be scoped to identity/read-only facts for the target's isolated
qualification or restoration phase. The helper does not alter provider policy.
Keep the source separately stopped/fenced before invoking `restore`, including
its import and temporary target lifecycles, and throughout any later target use.
Export resumes the source APP; the operator must perform and verify the separate
source stop under its lifecycle authority before a duplicate target is restored.
The helper does not establish or remotely verify exclusive live ownership.
For a seal-capable source artifact, the separate root `seal-source` command can
persistently block its local canonical entrypoint after export and clean stop.
See [SOURCE-SEAL.md](SOURCE-SEAL.md) for exact source/incarnation binding, the
fail-closed commit and separate recovery conditions. This local stop guard does
not prove remote fencing or turn logical copy/restore into a completed move.

## Required idle checkpoint contract

Before export, the operator completes one known application checkpoint across
committed SQL and declared files, then establishes write-idle operation: no
in-flight/uncommitted application transaction, queued SQL mutation, partial file
rewrite or other writer. The named small fixture must already be fully committed
and its files complete. Invocation asserts this operational precondition; the
helper does not implement an application checkpoint protocol or prove it itself.
Maintain this write-idle checkpoint through the separate verified source stop;
the completed export does not capture subsequent resumed-source writes.

SIGSTOP pauses APP instructions. It does not finish an in-flight transaction or
server-side statement. `--single-transaction` captures committed SQL; file bytes
can otherwise be ahead of SQL or half-written. Matching before/after SQL hashes
detect observed changes during capture but do not establish an atomic SQL/file
transaction. A completed export means capture under the declared operator
checkpoint contract, not arbitrary live-transfer coherence.

The target APP must likewise be write-idle during import, resume, adoption,
ordinary readbacks and the final owned-target stop. Unexpected startup/steady
state writes fail the strict comparison. Real rows are not excluded to force a
PASS; only the explicitly described machine binding and adoption audit differ.
Checkpoint creation and this target idle behavior remain native prerequisites to
observe for the selected fixture, not assumptions qualified by unit doubles.

## Explicit inputs

Install `logical-transfer.py` and `logical-transfer-native.py` together in a
root-owned, nonreplaceable directory. The native helper is passed to the existing
container's Python through local `podman exec`; it need not be built into the
image. Use an exact locally cached canonical image containing the adopted
entrypoint and matching immutable provenance, with no code overlays or anonymous
image volumes. The image is never pulled, rebuilt or imported by this command.
Export/restore retain the exact accepted adoption and monthly entrypoint pins
and additionally accept the seal-capable entrypoint SHA256
`9cd57eaa579201d6a9f4d3d4a10a4b59781c1d07db8c258b23ce2a5e5b479ac8`.
Only this new pin can authorize `seal-source`; see SOURCE-SEAL.md.
An older base image ID alone is insufficient, even if it was used successfully
with a temporary entrypoint mount in a separate procedure. Build/freeze the exact
derived artifact separately and use its resulting image ID for both source and
target. Neither that image's ID nor native qualification is inferred from source.

Supply a root0600 configuration JSON with exactly these fields (all values below
are placeholders supplied by the operator, not defaults):

```json
{
  "container": "<EXPLICIT_CONTAINER_NAME>",
  "image": "<64_HEX_CACHED_IMAGE_ID>",
  "etc_root": "<ABSOLUTE_OWNED_ETC_ROOT>",
  "sav_root": "<ABSOLUTE_OWNED_SAV_ROOT>",
  "provider_dir": "<ABSOLUTE_EXISTING_PROVIDER_DIRECTORY>"
}
```

Existing parents must be root-owned and not writable by group/others. Paths must
be absolute, without symlinks or overlapping etc/sav/provider roots. Source roots
already exist; target roots must both be absent. The provider directory/socket
has the canonical root:1102 ownership/modes and is mounted read-only. The helper
uses the ordinary local rootful Podman context, with no custom storage file,
container configuration file or engine mutation. Ambient DSNs, MYSQL_PWD,
Podman remote selectors and loader settings are not inherited. Podman is forced
local with `--remote=false`. Creation uses the ordinary native capability set,
networknone, pullnever, image-volume ignore, a60second stop timeout, fresh `/run`
tmpfs and the three explicit persistent/provider mounts. It adds no read-only
root, capability changes, memory/CPU/PID flags or host resource admission policy.

Every command requires `--max-payload-bytes <OPERATOR_BYTE_LIMIT>` as a positive
integer. This controls each SQL/diagnostic/file buffer, aggregate saved-file
contents, the manifest limit and the private message limit (four times this
value). It is an implementation buffer bound selected by the operator, not a
minimum host capacity, throughput guarantee or proof for larger future data.
For the first small fixture the native owner can select16777216 bytes explicitly.

Export from an already running source:

```sh
python3 logical-transfer.py --config <SOURCE_CONFIG> --max-payload-bytes <OPERATOR_BYTE_LIMIT> export \
  --output <ABSENT_PRIVATE_BUNDLE_DIRECTORY> \
  --saved-file <DECLARED_FILENAME> --saved-file <ANOTHER_DECLARED_FILENAME>
```

For an empty `sav/app`, omit `--saved-file`. The declared list must cover every
entry. The root0700 bundle retains `logical.sql`, a private `files/` directory and
`manifest.json` published last. Bundle files/dump/configuration remain root0600;
source file UID/GID/mode travel as explicit manifest fields, not transport modes.
The manifest binds SQL and file hashes/sizes/UID/GID/modes,
logical identity, row/schema inventories, image ID, artifact provenance and
configuration fingerprints. It contains no copied DB password/profile files.
The SQL/application contents remain sensitive operational data. Protect and
transport the bundle under the operator's own authority; no transport is built in.
Checksums establish consistency, not authenticity of an untrusted bundle.

Restore an existing completed bundle on a new target:

```sh
python3 logical-transfer.py --config <NEW_TARGET_CONFIG> --max-payload-bytes <OPERATOR_BYTE_LIMIT> restore \
  --bundle <EXISTING_COMPLETED_BUNDLE_DIRECTORY> \
  --receipt <ABSENT_PRIVATE_RECEIPT_FILE> \
  --adopt-restored-host-identity
```

Omit the adoption option for a same-host restore. A different provider-backed
host identity without this option refuses before SQL import. The command reuses
the supplied completed bundle and does not contact or re-export its source.
Successful output names a root0600 receipt with final stopped container ID,
comparison inventories, deliberate adoption delta and ordinary NODE API readback.
Starting the retained target for service is a separate operator lifecycle action.

## Failures and retained state

A failed export has no completed manifest. Source APP quiescence is released by
the native `finally` path and the host's independent resume attempt. Each SQL
operation is time/output bounded; a native whole-operation alarm is360seconds.
SIGINT/SIGTERM and handled exceptions take this cleanup path. SIGKILL, host loss
or a stopped runtime cannot provide an unconditional cleanup guarantee: inspect
the root-private runtime pause record and reconcile the exact PID/starttime before
resuming anything. Never send SIGCONT to an unverified or reused PID.

A restore failure retains its roots, database, canonical birth, private diagnostics,
container and failure receipt. The helper attempts to resume its APP and stop only
its own target; cleanup failure is recorded. A client timeout does not prove its
server-side SQL statement has stopped. The final owned-container stop is the
boundary that shuts down that target's private SQL server. If cleanup fails or
cannot be verified, the receipt warns that retained SQL state may still change;
do not treat it as a stable recovery image or retry against it.

A clean lifecycle requires this invocation's successful start, observed running
birth and matching container ID/engine StartedAt, followed by exit0, PID0 and a
stopped-clean birth. A new never-started container is recorded separately; its
exit0 and a previous stopped-clean birth cannot certify a new clean lifecycle.
The overall restore remains incomplete after that failure. No additional birth
ledger is created or edited by the helper. It never resets birth, repairs an
unknown datadir, overwrites existing roots or retries an import/adoption. Inspect
retained state before a separately authorized recovery. A failed receipt cannot
be used as a completed export. Cleanup does not mean rollback of imported SQL.
Only a newly created diagnostic from a successful call with empty stderr is
removed. Failure diagnostics, nonempty successful diagnostics and older files
remain private; no blanket diagnostic cleanup is performed.

This is a bounded planned manual logical restore, not continuous replication,
RAM migration, zero-downtime transfer, arbitrary-volume recovery, a complete move
CLI, production qualification or an HA promotion mechanism.
