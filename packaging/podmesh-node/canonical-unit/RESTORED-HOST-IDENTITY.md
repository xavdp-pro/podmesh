# Explicit identity adoption after a completed logical restore

This NODE-only administrative mode addresses a logical restore into a different
physical host identity. Ordinary startup still refuses a journal belonging to a
different host. `--recover-known-running` retains its separate existing behavior;
it cannot be combined with identity adoption. No MANAGER behavior is changed.

## Prerequisite and invocation

Use a fresh independent target created with the same qualified candidate artifact
that will perform adoption. Complete the root-authorized logical SQL restore into
its private application database and compare the imported schema, tables, rows,
objects and saved application state against the coherent source export. Preserve
the target's own datadir/birth identity, credentials, configuration and provenance.
Stop that target cleanly. A failed, partial, unrecorded or known-running birth is
not an identity-adoption input; do not edit a birth ledger to make it admissible.

The root operator places the retained completed export at
`/apps/podmesh-node/sav/admin/restore.sql` (root0600), then writes
`/apps/podmesh-node/etc/admin/restore-complete.json` (root0600):

```json
{
  "status": "restored-and-compared",
  "source_machine_sha256": "<SHA256 of the imported machine_id bytes>",
  "target_machine_sha256": "<SHA256 of the actual target host machine_id bytes>",
  "host_uuid": "<the logical NODE UUID retained from the source>",
  "export_sha256": "<SHA256 of the completed coherent SQL export>"
}
```

This protected completion request is the operator's explicit assertion that import
and equivalence verification have finished; the entrypoint verifies its export
binding and identities, and does not repeat a full SQL transfer comparison.
The request carries expected hashes, never a replacement provider fact.

Recreate the stopped owned target with fresh runtime and the explicit entrypoint
argument `--adopt-restored-host-identity`. The usual root PID1, clean-birth,
provenance, datadir, credential, schema and grant checks run unchanged. After its
private MariaDB is ready and before NODE starts, the entrypoint queries the real
root host provider under APP1102 and compares the observed identity with the
request. No environment override or container machine-id fallback supplies it.

One root SQL transaction compares the previous machine binding and logical NODE
UUID, updates only `metadata.machine_id`, and inserts one
`host_identity_adopt` observation. A failed compare updates/inserts nothing. A
native audit error aborts the non-force batch client and rolls back the InnoDB
identity change. The request/digests and audit timestamp are also retained in the
birth record. Other application rows, `host_uuid`, credentials and provenance are
not rewritten. NODE must subsequently pass its unchanged normal host binding and
readiness checks. Later starts use ordinary startup, not the adoption flag.

## Evidence and failure semantics

Exact SQL import equivalence precedes this deliberately authorized transition.
After adoption, the machine binding and one audit row intentionally differ; do
not claim all rows remain byte-identical. SQL UPDATE and audit are atomic in the
private database. The later birth-record write is not part of that SQL transaction;
a crash/error after commit remains an existing failed/partial admission requiring
separate recovery analysis. There is no retry, birth-record reset or silent repair.

The native test candidate `test_restore_identity_native.py` uses one explicitly
opted-in disposable MariaDB server and the real provider. It tests wrong source
identity/UUID, failed SQL compare-and-swap, audit-trigger failure with rollback,
and correct adoption with UUID/payload preservation and one matching audit. It
creates/drops only an initially absent `podmesh-node` test database. It does not
start or stop the server. The test is not full canonical container qualification.

This feature does not fence or stop a live source, promote an HA replica, migrate
RAM, implement replication, or prove production move semantics. A planned manual
restore must keep the source stopped under its external operational authority.
The retained failed114 fixture is not an input to this new mode.
