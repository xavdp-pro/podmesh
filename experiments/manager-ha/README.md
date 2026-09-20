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
| MH-16 | Append-only exchange audit | Typed phase rows, predecessor rules, checksums and immutable triggers fail closed | Tested locally |
| MH-17 | Authenticated-import receipt separation | Source plus wire operation map to a bounded destination-local `network:<sha256>` receipt ID; laboratory imports remain visible as unaudited | Tested locally; wired into the network crate's authenticated served path, not qualified on the laboratory hosts |
| MH-18 | Non-mutating preflight and inspection | v2 WAL files and mismatched stores retain their bytes; symlinks and unexpected schema objects refuse | Tested locally |
| MH-19 | Typed incomplete attempts | Locally allocated attempts remain distinct when a peer reuses a wire nonce; inbound and outbound unfinished phases are ordered and typed | Tested locally |
| MH-20 | Durable inbound decisions | Accepted imports and authenticated refusals follow separate checked signed chains; unsigned diagnostic writes and no-reply closes are distinct terminals and cannot become authenticated outcomes | Tested locally; wired into the network crate's authenticated served path, not qualified on the laboratory hosts |
| MH-21 | Bounded per-transaction verification | A process verifies a store completely at its first open and at each periodic pass; a transaction verifies the schema, the last verified rows, the rowids and the appended rows; any failure to read or verify a stored row closes the database file for the process, and no transaction commits on it afterwards; append, audit insertion, receiver pre-reply and export cost the same at 1,000 and 30,000 audit rows | Tested locally; laboratory soak pending |

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
them after reloading all stored facts inside the SQLite transaction of `observe`,
`import`, an authenticated import, `export`, `inspect` and `check_service`. An audit
record loads no fact: it reads the stored rows of its own attempt. Mutations and
audit records run IMMEDIATE transactions; `export`, `inspect` and `check_service`
run DEFERRED ones, read transactions that do not block writers. `Store::open` checks
identity and the last verified rows in an IMMEDIATE transaction, so opening a store
takes the write lock briefly.

Schema v3 stores a canonical topology/replica binding, immutable serialized
facts, typed mutation receipts and append-only typed exchange-audit events. WAL
plus synchronous FULL is requested only after a private-copy preflight has
accepted an existing schema and identity. Facts and receipts commit atomically;
an authenticated import also commits its accepted audit row in that same SQLite
IMMEDIATE transaction. Every insert must affect exactly one row. The exact
`sqlite_master` shape is verified before replay or mutation, and immutable tables
have no-update and no-delete triggers.

Stored rows follow a process-local integrity model, kept per database file (see
[`docs/MANAGER-PRE-REPLY-VERIFICATION.md`](../../docs/MANAGER-PRE-REPLY-VERIFICATION.md)).
The Store numbers the rows of each append-only table 1, 2, 3, … without a gap: it never
names a rowid and nothing deletes a row. The first open of a database file by a process
verifies every receipt, fact, audit row and attempt sequence, and that each table's rows
are exactly the rowids 1 to their count, in a read transaction. Every later transaction
verifies the schema shape, checks that the last verified row of each append-only table
is present with the same checksum (otherwise it verifies everything again) and that no
row precedes rowid 1, and verifies only the rows appended since, which must continue
the rowids, with the same per-row checks and the complete sequence of each attempt they
extend. A row that any writer adds without removing a stored row is therefore verified or
refused by the next transaction. The facts an operation loads, a replayed receipt or
audit event, and the stored rows of the attempt a new audit row extends are verified when
read; the candidate check reads those rows through the `UNIQUE(direction, attempt_id,
phase)` index. `Store::verify_full` repeats the complete verification in a read
transaction; long-running processes call it periodically
(`DEFAULT_FULL_VERIFICATION_INTERVAL`, 600 s). Any failure to read or verify a stored row
once a transaction holds its snapshot, or a schema found corrupt when the process opens
the file again, closes that database file for the rest of the process: later opens,
writes and exports return the same error, and every commit rechecks it under the lock
that records it, so no transaction commits afterwards. A busy or locked store, a failed
write or a failed commit closes nothing. The triggers refuse `UPDATE` and `DELETE` but
not `REPLACE`, whose implicit delete fires no trigger: an in-place change of an older row
that no operation reads, a replaced row or a removed row that leaves a gap included, is
detected at the next complete verification rather than by the next transaction. A table
whose last rows were removed is shorter and still verifies when no remaining row depends
on them.

Receipt checksums bind the logical manager, destination-local receipt identity,
receipt kind, source replica, wire operation, stored request and stored response
using the domain `podmesh-manager-ha-receipt/2`. Authenticated peer imports use a
bounded local identity `network:<sha256>` derived from a domain-separated tuple of
logical manager, source replica and wire operation. The wire operation remains in
separate receipt and audit fields. Local observations, laboratory imports and
authenticated imports therefore have explicit kinds. Canonical inspection lists
any import receipt that lacks a committed inbound-import audit.

Audit rows distinguish the locally generated `attempt_id`, wire nonce,
wire `operation_id`, local receipt ID and remote receipt ID. Attempt IDs use the
local `attempt:<sha256>` form. A wire nonce is either a validated decoded peer
nonce or, before decoding reaches one, a locally generated
`preauth:<64 lowercase hexadecimal SHA-256>` connection nonce. A pre-authentication
nonce remains stable within its local attempt and carries no peer, operation,
receipt, replay or effect authority. One shared sequence validator is used before
insertion, after loading, and while deriving incomplete attempts. An authenticated
accepted observation may lead either to an atomic accepted import or to a durable
typed refusal. The chosen decision must precede signed reply preparation, and a
completed signed reply write must reproduce its outcome, reason and receipt. A
complete or partially written unsigned diagnostic has its own exact-byte terminal
phase and cannot follow a durable decision or prepared signed reply. Partial
diagnostics retain their intended digest and announced body size, but use only
`unavailable` outcome/category; complete diagnostics retain the observed diagnostic
outcome. A no-reply close is a separate zero-byte terminal. None can become an
accepted result or authenticated refusal.
If storage prevents the terminal record, the network must close without a signed
response and no durable close is claimed.

Byte counts are per-phase actual transfer counts rather than cumulative attempt
totals. Request observation and signed or diagnostic reply-write phases carry
their respective actual bytes. Import, refusal-decision and close phases carry none. Prepared phases carry
zero actual bytes but retain bounded digest and announced-size intent. The schema
stores announced body sizes for both request and reply, so complete transfer
evidence requires exact framed-byte accounting. A transfer phase must retain
the digest and announced size of its corresponding prepared intent. Accepted
outbound completions also require the deterministic receipt ID that the
destination derives from the manager, source replica and wire operation;
non-accepted completions cannot assert a remote receipt.

An unavailable signed reply write records only a nonzero partial frame. Zero
bytes use `inbound_connection_closed`; a full signed frame uses only its accepted
or authenticated-refusal outcome. Signed refusal evidence is limited to peer
request reasons. Local configuration and preflight path-race failures are not
represented as signable request refusals.
Unavailable transport, reply-write and close audit rows use the stable
`transport_unavailable` reason. `unsafe_store` remains reserved for an actual
unsafe database or sidecar path. Neither is a signable authenticated
peer-request refusal.
Malformed and unauthenticated diagnostic audit rows use only the malformed
category with `invalid_request`; local store, identity, policy and transport
reasons are rejected both on insertion and when loading stored evidence.

The durable API returns typed `DurableError` values that distinguish `refused`,
`corrupt`, `storage` and `invalid_audit`; refusal reasons are a closed enum. These
classes are required by the future network layer so a local storage failure cannot
be signed as a durable peer refusal.

Existing v1 and v2 stores are refused without migration. On the first successful
open for a file-metadata/topology identity, preflight captures a stable private
copy of the database, WAL and SHM, opens only private files, and checks schema and
identity before opening the source read-write. If a copied WAL snapshot needs
materialization, SQLite opens and rewrites only a second private copy. A
process-local cache includes device, inode, size, modification time, change time,
replica and topology; in-place replacement and different identities do not match
the cache. A write operation that starts from a preflighted file state also records
the state its own commit and checkpoint left, so the next first open of that file by
a process finds it preflighted; a change made by anyone else still requires a new
preflight. A capture reads the files twice and keeps them only when both reads are
equal: into memory for a store whose files total at most 16 MiB, and above that by
copying them into the private directory and comparing a second read with the copy
through 1 MiB buffers, so a first open no longer holds twice the store in memory. The later source open uses SQLite `NOFOLLOW` and rechecks the path
metadata identity.

**Those file reads happen only while this process holds no SQLite connection on the
file.** POSIX advisory locks belong to a process and an inode: closing any
descriptor on that inode releases every lock the process holds on it, so a plain
read of a live database, WAL or SHM from a process that has it open through SQLite
drops that process's own locks (`How To Corrupt An SQLite Database File`, §2.2) and
lets another process delete the WAL under it, lose an acknowledged append or meet a
`disk I/O error`. Each database file therefore has a live-file entry — keyed by the
parent directory's device and inode and the file name — counting the connections
this process holds, with a gate held while a connection opens or a capture begins.
An open made while the process holds no connection preflights on a private copy as
above. An open made while it does copies nothing: it compares the file identity
(device, inode, birth time) with the file this process opened, refuses the open
untouched when it differs (`manager store path no longer names the database file
this process has open`, since the sidecars at those names may belong to the open
file), and otherwise checks the schema, identity and last verified rows in its own
`IMMEDIATE` transaction. A read-only inspection of a file this process has open
reads it through a read-only SQLite connection, in one read transaction, instead of
capturing it; a file no connection of this process holds is captured as before
(`tests/sqlite_locks.rs`). Tests preserve a killed-child v2 store with live
uncheckpointed WAL frames and a mismatched live v3 WAL store byte for byte.
Inspection and preflight reject symlinked or non-regular database and sidecar
paths through `lstat` before opening them. The subsequent regular-file open uses
Linux `O_NOFOLLOW|O_NONBLOCK` as defense against replacement races, and sidecar
names are constructed without lossy path conversion. Checksums detect corruption; they are not
signatures and do not protect against an administrator able to replace both rows
and checksums.

The executable is a one-request process boundary: database path, local
configuration path and replica ID are operator-supplied arguments; one JSON request
is read from stdin through EOF, and one JSON response is written to stdout. Exit
code 0 means success; exit code 1 accompanies an `error` object. A separate
external read-only form is available:

```sh
experiments/manager-ha/target/debug/podmesh-manager-ha-lab --inspect-store \
  DATABASE CONFIGURATION_JSON_FILE REPLICA_ID
```

It opens a stable private copy and never creates or changes the canonical store. The
resident's installed form also offers `--inspect-store --facts-only`, the durable
`inspect_facts_read_only`: the same private copy, verified only for the schema,
identity and facts, printing `history_count`, `ordered_facts` and
`logical_history_sha256`. Inspect a stopped store, or a live one through this
command, which captures the file by reading it twice and keeps the capture only when
both reads agree; never inspect a `podman cp` or `cp` of a live store, whose
database and WAL are copied one after the other while a writer commits and which is
torn (missing rows, rowid gaps, `database disk image is malformed`). There is no shell
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
snapshots. A complete verification is linear in retained facts, receipts and audit
rows; it runs at a process's first open, at each periodic pass and in `--inspect-store`.
The work of one transaction depends on the rows appended since the previous one and on
the retained facts, not on the retained audit rows (`tests/bounded_cost.rs`).
Full-snapshot import receipts can grow quadratically in this bounded laboratory; durable
incremental cursors, paging, retention quotas and large-history performance remain open. The API reserializes typed facts canonically; it does not
preserve original wire whitespace or claim compatibility with the registry
experiment's distinct observation envelope. Schema version is local SQLite
`user_version=3`; there is no negotiated remote protocol version.

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
never be copied as a replication mechanism, nor read as files by a process that has
them open through SQLite; tests copy only closed databases as explicit stale-backup
fixtures, and `tests/sqlite_locks.rs` holds the regressions for the locks.

## What this proves

The ten original model tests retain the MH-01–MH-08 contract. Twenty integrity tests
cover the MH-21 model: an edited old audit row refused by a new process's first open and
by the periodic pass, after which every operation fails closed; appended rows written by
another writer; an edited last verified row; replayed receipts and audit rows; the
per-attempt candidate check; a store kept open that trusts its own checkpoints but
not another connection's; rows added before a table's first row or after a gap, refused by
the next transaction; `REPLACE` in place found by the complete verification, and through
the primary key leaving a refused gap; unreadable facts, receipts and audit rows read at
use closing the store, while a busy store stays open; a schema found corrupt at open
closing the store; another file at a closed store's path verified completely before it
serves; and an operation waiting for the write lock while the store closes, which then
commits nothing. Added after the second review: rows added after a gap and rows moved
to another rowid, refused in every table; a rowid gap refused by the full inspection
where SQLite's integrity check reports `ok`; unreadable rows met by an authenticated
import and by an open; a closed file refused before anything reads it; a store this
process has open never copied again; another file at its path refused untouched. Four
unit tests cover the capture by copy of a larger store, a failure recorded only once
the commits in flight have finished, and a complete verification of a closed file that
does not count. Three lock tests (`tests/sqlite_locks.rs`) run child processes against
a store this process has open: commits stay visible to another process across a second
open, appends acknowledged around one survive a crash, and external readers and
`TRUNCATE` checkpoints close nothing. A bounded-cost test times append, audit insertion, receiver
pre-reply and export at 1,000 and 30,000 audit rows. The 69 executed process
acceptance tests exercise real compiled child processes and separate temporary
SQLite stores: restart, retry, sequence allocation, concurrent writers, disjoint
local writes, explicit JSON exchange, stale-copy return, conflict persistence,
reconciled eligibility, corrupt/fabricated receipt refusal, maximum imported revisions,
legacy WAL-schema refusal, typed exchange-audit corruption, namespaced receipts,
accepted/refused/complete-and-partial-diagnostic inbound chains, diagnostic peer
binding and partial-write boundaries, pre-authentication nonce authority limits,
transport-unavailable refusal separation, diagnostic reason/category closure,
per-phase frame accounting, read-only
external inspection, symlink refusal and bounded protocol input. The crash
acceptance test invokes its otherwise ignored child helper twice; that helper is
not omitted acceptance coverage.

The pre-commit crash helper holds a directly constructed SQLite transaction; the
post-commit helper calls the delivered Store API and is killed before publishing
its result. Independent SQLite connections inspect retained facts, receipt counts
and database integrity. These are specific process-kill windows, not every
instruction in the CLI transaction or a physical power-loss test. See
[EVIDENCE.md](EVIDENCE.md) for the executed checks and review boundary.

## What remains unproven

The authenticated-import API is wired into the network crate: its served path commits
an authenticated import and its inbound decision in one transaction
(`experiments/manager-network/src/lib.rs`). What that proves is a loopback
laboratory with static keys, not a deployment.
No complete authenticated origin or network transport, WireGuard integration, discovery,
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
