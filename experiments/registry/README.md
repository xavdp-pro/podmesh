# Offline observation-store foundation

This isolated Rust library explores the replicated manager's durable control-fact
history. It is not linked into PodMesh, shipped in its Debian package, or exposed
on a socket. It cannot invoke Podman, allocate an IP, publish DNS or activate a
standby. The governor's desired-state ledger remains separate.

Run `cargo test --locked --manifest-path experiments/registry/Cargo.toml` from the
repository root. The nested lockfile pins this experiment independently of the
installed service.

## Implemented contract

`Store::open` binds a local SQLite file to a mesh UUID and uses WAL with synchronous
FULL. `enroll` is an explicit local fixture action binding an epoch to a replica
and a resource-key prefix. An incoming event cannot enroll itself. This local
administrator profile is not cryptographic origin authentication.

`ingest` checks the original bytes against SHA-256 and validates a typed envelope;
unknown or duplicate fields, unsupported types, oversize bodies and unsafe numeric
values are refused. Only `observation.reported` is supported, under the distinct
`podmesh-registry-observation-lab/1` format. It does not claim compatibility with
the complete registry-event proposal. See `Observation` for the exact fields.
Reported timestamps are retained text, not validated freshness or health evidence.

`submit` transactionally persists exact bytes and an epoch-scoped request UUID.
An identical retry returns the same ID; altered bytes under the same request fail.
The caller supplies sequence and predecessor; automatic sequence allocation is
not implemented. A returned ID proves storage, not admission or runtime truth.

`states` rebuilds a read-only view from retained history. Missing predecessors or
dependencies remain pending. A sequence fork retains both bodies, quarantines the
whole producer epoch and transitively its consumers, including formerly admitted
consumers. Unrelated streams remain usable. No arrival-order winner is selected.
`export` returns exact bytes in deterministic ID order; it does not export local
enrollment as authority. Event updates and deletes are rejected by SQL triggers.

Limits: 64 KiB/event, 64 dependencies, 4096 events, 4096 request mappings, 16 MiB
combined body bytes, and 256 enrollment bindings. SQLite page/WAL overhead is not
included in the body limit. Exhaustion refuses new data; it never evicts history.
These are laboratory limits, not a claim of denial-of-service-resistant hosting.

## Qualification

The foundational tests cover exact-byte preservation, duplicate/changed requests, request quota
and retry at quota, restart persistence, shuffled three-store exchange, missing
predecessors, same- and cross-epoch fork quarantine, preservation of independent
observations, strict input refusals and immutable SQL history. Formatting and
strict clippy are part of the qualification commands.

Three stores in a process are not three deployed hosts. Clean close/reopen is not
a process-kill or power-loss test. No automatic HA or authenticated replication
claim follows from these tests.

## Next gates and feature reconciliation

| Scope | State |
| --- | --- |
| Durable immutable observation input and deterministic replay | Implemented/tested in this experiment |
| Local fixture enrollment, request deduplication, bounded retention | Implemented/tested |
| Manager/host runtime identity and writer epoch recovery | Pending |
| Automatic producer sequence | Pending |
| Frozen digest-snapshot cursors and bounded observation batches | Implemented/tested locally; no authenticated network endpoint |
| Grants, exclusive IP/placement conflicts, tombstones, migration lineage | Pending |
| Writer SIGKILL and acknowledged-commit persistence | Implemented/tested locally; not power loss |
| Stale snapshot recovery and control-plane rebuild | Pending |
| Signed origins, network transport, DNS, takeover and HA | Pending |

Three-pass review: governance boundaries remain explicit; operator-visible status
separates stored facts from authority; adversarial review found an unbounded request
mapping, corrected with quotas and a regression test. A separate Codex agent reviewed
the source, not Claude. Verdict: coherent with corrections for this foundation only.

## Stress qualification increment

The integration harness now kills an actual child writer with SIGKILL after 1,
10 and 100 acknowledged submissions. A separate SQLite connection checks database
integrity, acknowledged event rows, exact request mappings and absence of partial
request/event pairs before replay. Reopening and replaying the requests must keep
both counts unchanged. Additional unacknowledged commits are allowed. These are
three progress-triggered kills, not deterministic injection at every transaction
instruction or a power-loss test. ACK waits are bounded and failing tests kill/reap
their own child.

A second test exchanges 256 chained events across three separate SQLite files,
including disjoint offline subsets, reverse order, duplicate delivery and reopening.
It exercises persistence and catch-up, not network transport, independent allocator
writes, physical host loss or takeover. The ignored `crash_writer` test is a child
fixture explicitly invoked by its parent test; it is not skipped acceptance coverage.

Reconstruction now uses dependency queues rather than repeated whole-history scans.
Structural invalidity takes precedence over dependency quarantine; both prevent
admission, and their consumers are quarantined. Unit regressions also cover a
predecessor repeated in dependencies and invalid-predecessor propagation.

A SQLite page-allocation limit now injects SQLITE_FULL: both event and request
rows remain absent after failure, and a retry succeeds once the limit is lifted.
This is an allocation-failure injection, not filling a physical host disk. Four
concurrent connections submit the same request and converge to one event/mapping
with bounded caller-side retries for SQLITE_BUSY/SQLITE_LOCKED; the library does
not claim automatic retry.

Remaining stress gates: real storage exhaustion, production concurrency/retry
policy, stale snapshot recovery, authenticated inter-host exchanges, network
partitions, exclusive takeover and full control-plane loss. None is satisfied by
the local observation-store tests.

Final source counter-review found no blocking false positive in these scoped tests.
The concurrency case permits caller-side busy retries but does not assert that
contention actually produced a busy response. The SQLite allocation fixture uses
an in-memory database and may fail on the first insert: it does not cover every
partial-write location or physical WAL durability. Historical reruns remain evidence
of their own revisions, not extra independent test cases.

## Enrollment contention qualification

Enrollment already holds a SQLite IMMEDIATE transaction across the quota check
and insertion. A regression now holds an independent write transaction containing
the 255th binding while four connections attempt distinct enrollments. After that
writer commits, exactly one attempt succeeds, three receive the quota refusal,
and an independent connection reads exactly 256 rows. This validates the local
quota under writer contention; it does not establish distributed enrollment
consensus. See `NEXT-NETWORK-MILESTONE.md` for the next bounded exchange proposal.

## Transport-neutral exchange increment

`exchange::Snapshot` freezes a deterministic export and binds page offsets to its
mesh/content digest. Reuse that snapshot throughout pagination; a new capture
requires a new traversal. Pages contain at most 64 events and 256 KiB of original
body bytes, with a 600 KiB wire bound. Lowercase hex preserves exact whitespace and
UTF-8 bytes. These are laboratory limits, not a public endpoint.

`exchange::import` validates the whole batch framing before writes, then imports
individual events with existing local enrollment and quota checks. A batch can
partially succeed: its receipt identifies each event's persisted exact bytes and
separately its admission result. Admission is a point-in-time replay result and
can change when later events arrive. A receipt digest binds it to the exact input
batch; it is not a signature. Lost responses can be retried with identical bytes.
Cursor metadata is pagination context, never a receipt or authority claim.

Tests exercise bounded pages, duplicate delivery after a lost receipt, third-store
catch-up, exact-byte preservation, stale cursors, wrong mesh, unknown enrollment,
malformed/oversize input and per-event altered-body refusal. The exchange fixture
runs in-process with separate stores; it does not demonstrate authenticated network
transport or host-loss survival. Existing persistence/crash tests remain separate.

External authentication remains unimplemented: an operator-owned fixed peer/store
configuration, pinned transport identity and recipient executable must be specified
before wiring this module to SSH or another channel. No enrollment transfer,
manager takeover, DNS, IP allocation, collector or installed daemon is changed.


## Exchange review corrections and caller contract

Batches must carry strictly increasing, unique digest IDs and the frozen snapshot's
bounded event count. A non-final page must carry the exact next offset; it cannot
silently claim completion before that count. Page cursors refer to one frozen
snapshot; ingesting more data does not change it. Capture a new snapshot
and restart its traversal to discover later events. Compile-time body/event and
wire-overhead bounds guarantee one valid event fits; runtime guards refuse a
non-progressing page even if internal data violates those bounds. Tests cover
four 64-KiB events filling the body budget and a fifth on the next page, maximum
record metadata, and unchanged snapshot bytes after a new ingest.

**Inspect every event receipt before advancing.** `Receipt::advance` binds the
receipt to the original batch and requires every expected ID to be marked stored.
An event without an exact storage receipt keeps the cursor in place: retry only
transient storage contention, or reconcile a permanent validation/enrollment/quota
refusal. A replay result may report a refusal after its exact bytes were already
committed; advancing then acknowledges storage only, never admission. Do not silently
skip an unstored event because another event succeeded. The reference catch-up test uses this helper.
It checks completeness, not authenticity; transport authentication is still absent.

Per-event refusals expose only categories: validation, enrollment, quota,
retryable_storage (SQLite BUSY/LOCKED), or storage_fault (operator investigation,
including SQLITE_FULL). No SQL text, paths or raw database errors are returned.
A stored event can still report a replay failure; its storage receipt remains valid
while admission stays unknown. An outer import error can also occur **after partial
writes**, for example if the independent receipt query fails. On a missing/failed
receipt retain the batch and retry identical bytes; never assume rollback of the
entire batch. Only malformed framing is guaranteed refused before any writes.

Enrollment uses IMMEDIATE transactions to serialize its quota check and insertion.
Existing ingest/submit use DEFERRED transactions: competing writers can encounter
BUSY/LOCKED while upgrading a read transaction. The library does not retry. A caller
must bound retries of identical inputs; exclusive enrollment retries must never
weaken quota checks. No distributed-lock semantics follow from SQLite transactions.

The manifest declares Rust 1.88 because fixed-size slice `as_chunks` requires it.
This is a source-level minimum, not evidence of a full MSRV build; qualification
used the available toolchain. `--locked` pins dependency resolution to Cargo.lock;
it does not pin the Rust compiler, operating system or system environment.
