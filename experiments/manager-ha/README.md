# Replicated logical manager laboratory

Status: isolated deterministic model plus a SQLite-backed local process API.
It is not a deployed manager, a network replication protocol, a production
failover service or proof of high availability.

## Objective

PodMesh calls the former "manager" the **control-services universe**. It is one
logical manager with one distinct replica per host. The replicas retain control
facts locally so that healthy hosts do not depend on one shared live filesystem.
This experiment makes the smallest merge and activation rules executable without
changing the installed PodMesh service or the durable registry experiment.

Design inputs inventoried for this boundary:

- `INTENT.md`: human-agent operation contract and incomplete HA boundary;
- `docs/CONTROL-SERVICES-UNIVERSE.md`: replicated logical identity, partition,
  reconnect, conflict, coordinator and bootstrap rules;
- `docs/NETWORK-AND-PLACEMENT.md`: stable universe/IP identity and non-overlapping
  allocation pools;
- `docs/RUST-IMPLEMENTATION-PLAN.md`: manager bootstrap and partition acceptance;
- `experiments/registry/README.md`, `STRESS-PLAN.md` and
  `NEXT-NETWORK-MILESTONE.md`: durable observation-store evidence and the explicit
  absence of grants, takeover, authenticated transport, DNS and deployed HA.

The model follows these rules:

- every replica and host has a stable, distinct identity;
- every disconnected-write scope has exactly one declared replica owner;
- slash-delimited parent and child scopes are treated as overlapping and cannot
  be delegated to different replicas;
- during a partition, a replica may append observations only inside its own
  scope; unreachable state never becomes proof of failure;
- immutable histories are exchanged before coordinator selection;
- the lowest stable replica ID is only a deterministic coordinator tie-breaker;
- subject revision and predecessor relations select current facts independently
  of coordinator identity, so a lower-ID coordinator does not become a truth rule;
- competing heads and duplicate active claims for an exclusive resource remain
  visible and blocked;
- no replica advertises an active service or IP without an explicit permit made
  from a fully reconciled view; only the permitted replica advertises it, and any
  later history change invalidates that laboratory permit.

## Executable feature inventory

| ID | Capability | Acceptance evidence | State |
| --- | --- | --- | --- |
| MH-01 | One logical manager, one replica per distinct host | Duplicate host and replica topology is refused | Tested locally |
| MH-02 | Pre-delegated non-overlapping partition writes | Two separated replicas record independent scopes; cross-scope write is refused | Tested locally |
| MH-03 | History exchange before coordination | Diverged replicas cannot reconcile; converged replicas select the lowest stable ID | Tested locally |
| MH-04 | Causal fact reduction is independent of coordinator choice | A lower-ID stale copy cannot reconcile before exchange; revision two from another owner remains current afterward | Tested locally |
| MH-05 | Exclusive conflict blocking | Two universe claims for one IP retain both events and block that IP | Tested locally |
| MH-06 | Inactive replica silence | A reconciled active claim permits only its owning coordinator; another owner, quarantined claim or later history change cannot reuse the permit | Tested locally |
| MH-07 | Stale replica catch-up | A stale third copy imports the missing revision and materializes the current head | Tested locally |
| MH-08 | Incomplete or forked history quarantine | Missing predecessor, same-revision fork and overflowing imported revision relation block the affected subject | Tested locally |
| MH-09 | Durable replica identity and immutable history | Separate CLI processes retain identity, revisions and SHA-256-checked rows; mismatched identity and corrupt history fail closed | Tested locally |
| MH-10 | Atomic mutation and retry | Fact and checksummed operation receipt commit together; verified retries return the original result; corrupt receipts, incompatible reuse and partial invalid imports fail | Tested locally |
| MH-11 | Process interruption recovery | Child killed during a SQLite transaction rolls back; child killed after Store commit before reply replays once | Tested locally; no power-loss proof |
| MH-12 | Concurrent process writers | Eight child processes allocate distinct sequential events without lost writes | Tested locally |
| MH-13 | Persisted stale-copy catch-up | Offline old database backup catches up from retained peer facts before new sequence allocation | Tested locally; lost receipt recovery remains open |
| MH-14 | Process API reconciliation and conflict gating | Three independent stores exchange JSON facts, block divergence/conflicts, and report at most one eligible replica for one reconciled history | Tested locally; not activation/fencing |
| MH-15 | Typed bounded local process interface | Unknown fields and input larger than 1 MiB fail before database creation | Tested locally |

Run:

```sh
cargo test --locked --manifest-path experiments/manager-ha/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-ha/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-ha/Cargo.toml -- --check
```

## Durable process API

SQLite plus an append-only event history is selected **for this isolated laboratory
increment**, as authorized by the implementation task. This does not select a
production storage standard or change the installed PodMesh service. The existing
reducer and reconciliation rules remain the contract; `durable::Store` invokes
them after reloading all stored events inside each SQLite IMMEDIATE transaction.

Schema v2 stores a canonical topology/replica binding, immutable serialized facts
with SHA-256 checksums, and immutable checksummed local operation receipts. WAL plus synchronous
FULL is requested on every open. Facts and receipts commit atomically before a
mutation response is returned. Reloading under the write transaction prevents
concurrent processes from overwriting stale in-memory state. Invalid imports roll
back as a batch. SQL triggers refuse updates/deletions; they do not protect against
an administrator who can replace the database or schema. Hashes detect accidental
byte changes; they are not signatures.

Every request verifies all receipt checksums inside the same transaction before
reading state, replaying a response or writing another event. Each checksum is
SHA-256 over the UTF-8 serialization of this JSON string array:
`["podmesh-manager-ha-receipt/1", operation_id, request_json, response_json]`.
JSON escaping and fixed array positions make the framing unambiguous, including
embedded quotes, delimiters, newlines and NULs. The stored request/response JSON
strings are bound exactly. A bad checksum blocks the API without a false replay.
An administrator able to alter both content and checksum can fabricate a matching
row: this is integrity detection, not a MAC, signature or authentication mechanism.

Schema v1 lacked receipt integrity data and is refused without mutation. There is
no automatic migration that could bless already corrupted receipts with newly
computed checksums. This unreleased laboratory increment uses fresh v2 fixtures;
recovery or migration of older files requires a separately reviewed procedure.
Revision successor arithmetic is checked both for local writes and imported causal
relations. Malformed `u64::MAX` predecessors are quarantined rather than overflowing;
restarting the process preserves the quarantine.

The executable is a one-request process boundary: database path, local
configuration path and replica ID are operator-supplied arguments; one JSON request
is read from stdin through EOF, and one JSON response is written to stdout. Exit
code 0 means success; exit code 1 accompanies an `error` object. There is no shell
execution, daemon, network listener, or implicit remote connection.

Example using a disposable operator-owned directory:

```sh
cargo build --locked --manifest-path experiments/manager-ha/Cargo.toml
mkdir -m 700 /tmp/manager-ha-example
cat > /tmp/manager-ha-example/configuration.json <<'JSON'
{
  "logical_manager_id": "example-manager",
  "replicas": [
    {"replica_id": "r1", "host_id": "h1"},
    {"replica_id": "r2", "host_id": "h2"},
    {"replica_id": "r3", "host_id": "h3"}
  ],
  "grants": [
    {"scope": "scope1", "owner_replica_id": "r1"},
    {"scope": "scope2", "owner_replica_id": "r2"},
    {"scope": "scope3", "owner_replica_id": "r3"}
  ]
}
JSON
printf '%s\n' '{"operation":"observe","operation_id":"example-1","scope":"scope1","subject":"universe","exclusive_resource":null,"active_claim":false,"value":"observed"}' |
  experiments/manager-ha/target/debug/podmesh-manager-ha-lab \
  /tmp/manager-ha-example/r1.sqlite /tmp/manager-ha-example/configuration.json r1
printf '%s\n' '{"operation":"export"}' |
  experiments/manager-ha/target/debug/podmesh-manager-ha-lab \
  /tmp/manager-ha-example/r1.sqlite /tmp/manager-ha-example/configuration.json r1
```

| Request | Fields beyond `operation` | Result |
| --- | --- | --- |
| `observe` | `operation_id`, `scope`, `subject`, `exclusive_resource`, `active_claim`, `value` | Durable fact, including allocated sequence and predecessor |
| `export` | None | `snapshot` containing configuration, replica ID and full immutable facts |
| `import` | `operation_id`, `snapshot` (the exported inner object) | Newly inserted count and history length |
| `inspect` | None | Current heads, conflicts, blocked exclusive resources, retained event count |
| `check_service` | `snapshots` from every other declared replica, `exclusive_service` | Coordinator ID and `eligible_in_supplied_history` |

Configuration and request inputs each have a 1 MiB limit. Exchange uses complete
snapshots; durable incremental cursors, paging, retention quotas and large-history
performance remain open. The API reserializes typed facts canonically; it does not
preserve original wire whitespace or claim compatibility with the registry
experiment's distinct observation envelope. Schema version is local SQLite
`user_version=2`; there is no negotiated remote protocol version.

An `import` cannot change the local topology or identity. The supplied configuration
must match the already declared topology; it is comparison data, never enrollment.
`check_service` always includes the current local database, requires every other
replica exactly once and requires byte-equivalent typed histories. It never accepts
a caller's old local snapshot instead of current local state. Its output is a
**laboratory eligibility observation about supplied snapshots**, not a transferable
permit, durable authorization, live-peer acknowledgment or action. Peer snapshots
can be stale or fabricated; they are not authenticated. No service is advertised.

Retrying a committed mutation replays its original response, which can describe
historical state; use `inspect` for current state. Receipts are local and are not
replicated. Restoring an old backup may lose newer receipts even after peer facts
catch up, so deduplication across rollback of the local database is **not proven**.
An offline copied database cannot be rebound to a different configured replica,
but a second copy using the same identity is not fenced. Live SQLite files must
never be copied as a replication mechanism; tests copy only closed databases as
explicit stale-backup fixtures.

## What this proves

The ten original model tests retain the MH-01–MH-08 contract. The seventeen process
acceptance tests exercise real compiled child processes and separate temporary
SQLite stores: restart, retry, sequence allocation, concurrent writers, disjoint
local writes, explicit JSON exchange, stale-copy return, conflict persistence,
reconciled eligibility, corrupt/fabricated receipt refusal, maximum imported revisions,
legacy schema refusal and bounded protocol input. The crash
acceptance test invokes its otherwise ignored child helper twice; that helper is
not omitted acceptance coverage.

The pre-commit crash helper holds a directly constructed SQLite transaction; the
post-commit helper calls the delivered Store API and is killed before publishing
its result. Independent SQLite connections inspect retained facts, receipt counts
and database integrity. These are specific process-kill windows, not every
instruction in the CLI transaction or a physical power-loss test. See
[EVIDENCE.md](EVIDENCE.md) for the executed checks and review boundary.

## What remains unproven

No authenticated origin or network transport, WireGuard integration, discovery,
DNS, Podman control, real service IP, leases, clocks, failure detector, fencing,
physical power-loss survival, installed host deployment, Logger integration or
end-to-end HA is implemented or proven. Grants are static fixtures; observation
and receive timestamps, evidence references, producer epochs, dynamic membership,
revocation, bounded incremental exchange and backup/receipt recovery need separate
contracts before the design brief's complete event format is implemented.

Coordinator selection compares stable replica IDs bytewise; freeze the production
ID/priority format before treating it as creation order. Reconciliation does not
choose the newest truth by coordinator rank. Availability of one copy does not
prove a peer stopped. No quorum-free takeover, split-brain prevention on real
hosts, zero data loss or production availability claim follows from this lab.

The next integration step is a reviewed mapping to the existing durable registry
envelope and exchange contract, explicit origin/enrollment authentication and
recovery identities. Only then qualify actual control-service replicas under
partition, reconnect, stale return and host loss, with separate exclusive-effect
fencing and independent runtime observation.
