# Explicit local source seal for a planned cold NODE handoff

This root command closes the local stale-source restart gap in the planned manual
logical-transfer contract. It does not transfer a running workload, select a
winner, fence another host or implement HA/quorum. Native execution remains a
separate qualification of the exact adopted artifact.

Use the exact seal-capable entrypoint and both corresponding helpers. Their
source hashes are fixed in the helpers; there is no operator-provided hash bypass.
An ordinary source must have been born with that artifact. Accepted older
entrypoint pins remain supported by export/restore, but cannot authorize sealing.
Their old bundles lack current-incarnation binding. Existing stopped births also
continue to reject an unapproved change of immutable provenance; this feature
provides no upgrade or retroactive adoption procedure.

Under one exclusive root/lifecycle maintenance window:

1. Establish the existing committed SQL/file checkpoint and keep the source
   write-idle. Perform one completed coherent export with the exact helpers.
2. Stop that source cleanly using its existing lifecycle authority. Verify the
   actual container ID, engine incarnation, PID0/exit0 and stopped-clean birth.
   Maintain checkpoint idleness from export through stop: resumed-source writes
   are outside the bundle, and a clean stop does not prove their absence.
3. Run the same host helper with the original explicit root0600 source config:

   ```text
   logical-transfer.py --config SOURCE_CONFIG --max-payload-bytes BYTE_LIMIT \
     seal-source --bundle COMPLETED_BUNDLE --receipt NEW_PRIVATE_RECEIPT
   ```

4. Independently verify the durable source marker and receipt before authorizing
   restore or writable use of the target. Keep the old source sealed throughout
   target use. The restore helper does not remotely verify or assert this step.

No stop, SQL, resume, target creation, resource change or implicit lifecycle
operation is performed by `seal-source`. It uses protected filesystem reads and
local engine inspection. It requires the completed bundle and validates its SQL,
files and directory digests through the existing loader, exact source config and
cached image ID, exported container ID/StartedAt, per-start lifecycle ID, datadir
inode/device, stopped birth digest, unchanged credential/profile fingerprints,
logical NODE UUID, provider-bound machine digest and immutable provenance.
It reuses the closed mount/entrypoint contract and exact helper digest. It requires
the current source entrypoint's exact fixed hash, not merely a claimed image ID.

New exports from seal-capable artifacts carry a `source` metadata object in the
existing root-private v1 manifest. This additive metadata is not a transferred
admin tree. The per-start lifecycle ID is stored before starting the database;
a later ordinary or recovery start changes that ID and invalidates the old seal
request, even if the container ID or paths are reused. Older bundles remain
restorable without rewriting them and are refused only by `seal-source`.

The command cooperatively locks the existing export lock, rechecks stopped state,
and commits `sav/admin/source-seal.json` with O_EXCL, file fsync and parent fsync.
It never edits birth.json or overwrites an existing seal. The private marker binds
the exact manifest digest (therefore every declared SQL/file/directory digest),
source identity/configuration/incarnation and stopped evidence. A separate new
receipt is written outside the source/bundle roots after commit.

All entrypoint modes check marker presence before layout, diagnostics, ledger
loading or child startup. Ordinary start, `--recover-known-running`, restored-host
adoption and monthly construction have no bypass. Empty, malformed, symlink or
directory markers fail closed. The existing birth ledger is preserved on refusal.
Missing admin state on a fresh target is allowed. App cannot create/remove the
marker because the admin directory is root0700.

A write/fsync/receipt failure is an ambiguous administrative commit. Preserve
all files, inspect the actual marker and stop; absence of a success response is
never permission to restart the source or blindly repeat a handoff. A partial
marker still blocks startup. No automatic unseal, rollback, deletion or broad
force flag exists. The source might already be stale if a target became writable.

Recovery is a separately authorized operator action. Before considering an old
source for service, retain the original seal/bundle/receipts; identify every target
by exact container/incarnation/state roots and establish that no target can still
write, including its children/provider ownership and restart authority. Determine
which copy is authoritative and whether any target accepted post-export writes.
If it did, the old exported state is not a safe rollback: reconcile or restore the
chosen latest state under a separate procedure. Revalidate source immutable
provenance, credentials/profile, datadir and selected checkpoint, then obtain
explicit recovery authority for that exact state. This feature intentionally
provides no command that silently removes the source seal.

Logical restore creates new target etc/sav roots and transfers only application
SQL, flat sav/app regular files and four empty bootstrap directory descriptions.
It does not copy sav/admin, source credentials, birth or source-seal.json. The new
manifest's source binding stays private bundle metadata; target adoption and fresh
credentials retain their existing behavior. Copying an entire source sav tree is
outside that format and would correctly carry the seal and block startup.

The marker protects the guarded canonical entrypoint on the preserved source
mounts under the operator's exclusive maintenance contract. Root can bypass code,
replace mounts or launch an older artifact; this is not protection against a
competing privileged administrator, storage rollback or an uncooperative host.
A snapshot of stopped engine state is not a distributed exclusion protocol.
