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

Ten tests cover exact-byte preservation, duplicate/changed requests, request quota
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
| Automatic producer sequence, stream cursors, bounded batch protocol | Pending |
| Grants, exclusive IP/placement conflicts, tombstones, migration lineage | Pending |
| Snapshot recovery and actual process-kill proof | Pending |
| Signed origins, network transport, DNS, takeover and HA | Pending |

Three-pass review: governance boundaries remain explicit; operator-visible status
separates stored facts from authority; adversarial review found an unbounded request
mapping, corrected with quotas and a regression test. A separate Codex agent reviewed
the source, not Claude. Verdict: coherent with corrections for this foundation only.
