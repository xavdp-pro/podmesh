# Resident control-services replication laboratory

Status: local executable laboratory, no deployment or HA claim. One logical
manager retains control facts in distinct replicas, without a shared live
filesystem. This increment runs periodic authenticated snapshot exchange using
the typed `manager-network` and durable SQLite `manager-ha` path dependencies.
It never calls Podman, runs commands, publishes DNS/IP, grants permits, performs
fencing or activation, or changes enrollment.

## Resident behavior

A persistent TCP listener admits a fixed number of workers. Each calls the
transport's `Node::serve_connection` for exactly one authenticated, bounded frame
and atomic import. Excess connections are closed and counted without an application
queue; the kernel has its own finite backlog. One outgoing worker visits the exact
configured peers sequentially, with independent bounded exponential backoff.
Successful exchange resets the peer interval. An unreachable peer does not become
a dead host or an activation decision. A slow peer delays others by its bounded
attempt; hostile admission fairness is not guaranteed.

Operation IDs and nonces use OS randomness. The same observed snapshot reuses an
operation ID, avoiding a new receipt for each unchanged poll. After restart, IDs
are fresh. The outgoing worker records the digest of the snapshot each peer last
acknowledged with an authenticated receipt: it exchanges again when the local snapshot
differs, and otherwise only after the unchanged-snapshot refresh delay, so idle replicas
add no audit rows at every interval. The acknowledgement state restarts empty, and a
peer restored from an older store receives the local facts at the latest after that
delay. The transport exports again inside its call: an incoming import between
the scheduler's snapshot digest and that export can cause a transient replay
binding refusal. A changed digest creates a fresh ID on the next attempt. The
transaction refuses mismatches rather than making an unsafe mutation.

SQLite retains the dependency's WAL/FULL, immutable history and receipt checks,
identity binding and atomic import. The resident's first store open verifies the whole
store before the listener and control socket exist; a background worker repeats that
complete verification, in a read transaction, every full-verification interval, and
writes a failed pass to standard error. Any failure to read or verify a stored row, in a
pass or in any transaction, closes that database file for the process, and no later
transaction of the process commits on it: appends answer `append_observation_uncertain`,
exchanges fail, status and shutdown stay available, and a restarted resident refuses to
start on that store. The closed state belongs to the database file, not to its path: a
different file placed at the configured path is verified completely at its next open by
the process before it is served. A held lock file prevents two residents on the
same configured database path. It does not fence copied databases, path aliases
or a privileged actor. No history/receipt compaction or disk quota exists yet.

## Configuration and bounds

The Cargo executable remains `podmesh-manager-resident-lab`; it is intended to be
installed as `podmesh-managerd`. `--version` prints the installed-facing identity
`podmesh-managerd 0.1.0`, independently of the filename. This is the crate version,
not an assertion about a Debian package version or installed deployment.

The package candidate accepts exactly:

```text
podmesh-managerd --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager --runtime-dir /run/podmesh-manager [--validate-config]
podmesh-managerd --inspect-store [--facts-only] --config /etc/podmesh-manager/config.json --state-dir /var/lib/podmesh-manager
podmesh-managerd --version
```

The runtime and validation forms require all three path flags; `--inspect-store`
requires only `--config` and `--state-dir`, and rejects `--runtime-dir` and
`--validate-config`; `--facts-only` is accepted only with `--inspect-store`. Flags can
appear in any order. Unknown, repeated, mixed positional, missing-value and
missing-required flags are refused. The one
positional `CONFIG.json` laboratory form remains available;
its absolute state/runtime boundaries are derived from the configured DB/socket
parents and undergo the same checks. All runtime forms require the explicit
environment value `PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers`.
Missing mode or `disabled` refuses runtime networking before DB, lock or socket
creation. Unknown modes are refused. `--validate-config` works with disabled or
enabled mode but never binds, opens/creates a durable database, takes a lock or
creates a socket. It reads the config and filesystem metadata only; a successful
offline check does not validate an existing SQLite file's identity/integrity.

Paths must be absolute, at most 4096 bytes, without `.`/`..`, repeated/trailing
separators or any symlink in their checked ancestry. The DB and socket must be
direct children of declared state/runtime directories. State/runtime directories
must exist and belong to the effective user: state permits group read/traverse
(package mode `0750`), runtime permits no group/other access (`0700`). Their
ancestors must belong to root or the effective user and disallow other-user
writes, except root-owned sticky temporary directories. Config files must be
regular, singly linked, root- or effective-user-owned and not group/other writable;
root-owned group-readable `0640` config is supported. Existing DB/lock/WAL/SHM
files must be singly linked regular files owned by the effective user and not
group/other writable. Existing sockets must be private and owned by that user.
Validation follows no symlink and performs no automatic directory creation.

These metadata checks assume the effective account and root do not race path
replacement after validation; descriptor-relative race hardening against those
trusted actors is not implemented. Do not run this service under an account
shared with untrusted software. The package networking default stays disabled;
its systemd address-family/firewall policy needs separately reviewed changes
before opting into real networking.

Unknown JSON fields are refused. The required configuration has `network` (the
complete transport `ConfigurationFile`), `control_socket`,
`observation_writer_uid`, `interval_ms`, `max_backoff_ms`, and
`incoming_workers`. `observation_writer_uid` fails closed when absent. Two fields
are optional and omitted when serialized unset: `full_verification_interval_ms`
(default 600,000) and `unchanged_snapshot_refresh_ms` (default 60,000). The
private socket is `0600`, so the current package-facing boundary normally makes
UID 0 or the service account its only reachable writer. Shared group/ACL writer
admission is deferred to Stage P; this service does not weaken the socket mode.
`network` fixes the replica identity/topology, local DB, TCP bind and all exact
peer endpoints with distinct pair keys. Requests cannot select paths, keys,
endpoints, commands or topology. Protect local config/DB; never commit secrets.

| Boundary | Limit |
| --- | --- |
| Local config | 1 MiB before parsing |
| Peers | At most 15, all other topology members exactly once |
| Incoming workers | Configured 1–8; tests use 2 |
| Outgoing workers | 1 |
| Control handlers | 1, serialized in main loop |
| Network request/reply | 512 KiB frame |
| Network read/write | Absolute 2-second deadline per frame, including trickled bytes |
| Connection retry | 2 seconds |
| Periodic interval | 100–60,000 ms |
| Maximum backoff | At least interval, at most 300,000 ms |
| Complete store verification | Optional, 1,000–86,400,000 ms; default 600,000 ms |
| Unchanged snapshot refresh | Optional, at least interval, at most 3,600,000 ms; default 60,000 ms |
| Control request frame | 32,768 bytes within the shared 250 ms control deadline |
| Observation value | Nonempty UTF-8, at most 4,096 bytes |
| Control response | 32,768 bytes within the shared 250 ms control deadline |
| Socket path | Absolute, at most 100 bytes, private parent directory |

SQLite uses a five-second busy timeout. These are not hard real-time guarantees.
A transaction verifies only the rows appended since the previous one, so an append, an
exchange and every store open after the first cost the same whatever the number of
retained audit rows; they still grow with retained facts, which every snapshot carries. The first open of a
resident, and each background verification, remain linear in the whole store. Storage/OS
can stall.
Oversize snapshots fail to exchange; history is never truncated into a success.

## Private control and observation

The Unix socket is mode `0600` in a directory without group/other permissions.
Send one JSON object and close the write half. Three requests exist:

```json
{"operation":"status"}
```

```json
{"operation":"shutdown"}
```

```json
{"operation":"append_observation","operation_id":"op-1","scope":"team/service","subject":"subject-1","value":"observed text"}
```

`append_observation` admits no other fields. Before any Store access it reads
Unix peer credentials and requires the configured UID. It accepts operation IDs
and subjects made of ASCII letters/digits plus `-_.:`, 1–128 bytes; scopes use
that alphabet in nonempty `/`-separated segments, also 1–128 bytes, with no
leading/trailing slash or `.`/`..` segments. `network:` operation IDs are
reserved. It constructs only `Request::Observe` with no exclusive resource and
`active_claim=false`, then returns the durable `execute_with_receipt` result.
Malformed JSON, wrong types and unknown fields return `invalid typed control request`;
an oversized frame returns `control request bound exceeded`. Invalid append values,
UID or policy failures, and durable policy refusals return
`append_observation_refused`. An occupied worker returns
`append_observation_busy`; it admits no new work and makes no claim about whether
a prior request with that operation ID will commit. `status_unavailable` is a
bounded diagnostic failure.

A missing reply, a partial JSON reply or `append_observation_uncertain` leaves the
client outcome uncertain; retry only the identical operation ID and content.

The resident admits one append worker. The original server accept-to-response
budget is an absolute 250 ms deadline, shared by read, worker wait and write. If
SQLite work has not finished before the reserved response window, or a storage
outcome cannot be classified as a durable refusal, the request returns
`append_observation_uncertain`. The worker remains joined during shutdown
and can still commit; retry the identical operation ID to obtain its original
receipt/replay. A changed request under that ID is refused. SQLite work itself is
not cancellable, so a shutdown response can be prompt while final process drain
waits for an admitted append worker.

Live `status` is a bounded resident diagnostic: replica identity, peer diagnostics,
admission counters and `activation_authority=false`. It never opens, copies or
serializes the durable store, and returns
`canonical_inspection_available_via: "--inspect-store"`. Full canonical facts,
receipts, audits, integrity verification and source-copy behavior are exclusively
provided by `--inspect-store`; status remains bounded as audit retention grows.
`--inspect-store` performs this read-only inspection without network mode,
listener, socket, worker, interval or peer-key validation. Its output carries every
receipt and audit row, 21 MB at 21,700 audit rows and 96 MB at 100,000. `--inspect-store
--facts-only` makes the same private copy, verifies only the schema, identity and
facts, and prints `history_count`, `ordered_facts` and `logical_history_sha256`, with
the full inspection's values: its output and verification do not grow with the audit
table, and the administration app's origin reads facts through it. It reads no receipt
or audit row, so it is not an integrity verdict on the store. It validates only
config parsing, bounded manager/topology identities, local replica membership and
the declared existing state-store path; it never creates, migrates, repairs or opens the store
read-write. The durable inspector writes a private full source copy below
`$TMPDIR`, and its current path ownership policy requires the caller to own the
state directory. A root operator cannot use it directly on a service-account
state directory without an explicit, separately reviewed access arrangement.

`status` and `shutdown` rely on the private `0600` socket rather than a separate
peer-UID check. If Stage P later broadens socket access for writers, those writers
would also be able to request shutdown; Stage P must define separate admission if
that is unacceptable.

Status includes peer counters, last authenticated success age, next retry delay,
acknowledged history count, local history count observed before attempt, and
`history_count_delta`. This is not exact causal lag
or convergence proof: equal counts can differ, and replayed receipts describe a
historical committed result. Unknown values remain null. Unsigned diagnostics
and failed exchanges clear `history_count_delta` to null; the previous acknowledged
count is retained only as historical data with its last-success age. A new local
count is never compared with an unreachable peer's old count as a current lag.
Unsigned diagnostics remain explicitly `unauthenticated_remote_diagnostic` and
verified remote refusals explicitly `authenticated_remote_refusal`; no text
inference or authenticated authority is derived from either. `activation_authority`
is always false. Peer status restarts unknown.

Shutdown stops scheduling/admission and removes the owned private socket before
fallible worker joins, then drains and joins workers. Control status failures, append storage failures, append-worker panics, and
transient outgoing Store/open failures become bounded replies, a bounded failure
counter, or per-peer backoff rather than terminating the resident. Every started worker is
joined even when another worker fails. Abrupt death leaves a stale
socket: restart refuses it until the operator verifies process death and removes
that exact private path. Tests do so only after collecting the killed child's exit.
There is no automatic stale-file cleanup or lost-identity recovery procedure.

TCP and Unix reads/writes retry interrupted system calls against the original
absolute deadline; an interrupt never renews the deadline. No signal injection
test is claimed for this retry branch; frame deadline and process tests cover the
surrounding I/O paths.

## Validation and gaps

[EVIDENCE.md](EVIDENCE.md) covers three simultaneous compiled processes, UID-bound
nonexclusive observations, live TCP partition/reconnect, SIGKILL/restart,
conflicts, wrong keys, authenticated invalid batches, raw-frame/value bounds,
read-only inspection, framing/admission limits and shutdown. The partition test
uses six TCP proxies to cut both directions around one still-running resident.

Remaining: key rotation/revocation, signed original provenance, encryption,
WireGuard/real-host qualification, bounded incremental history, retention/quota,
receipt/backup recovery, identity fencing across paths/hosts, Logger, integrated
control-services deployment and actual exclusive-effect fencing. There is no
automatic HA, quorum-free takeover, zero-loss or production availability claim.

```sh
cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-resident/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-resident/Cargo.toml -- --check
```
