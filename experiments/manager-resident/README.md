# Resident control-services replication laboratory

Status: local executable laboratory, no deployment or HA claim. One logical
manager retains control facts in distinct replicas, without a shared live
filesystem. This increment runs periodic authenticated snapshot exchange using
the typed `manager-network` and durable SQLite `manager-ha` path dependencies.
It never calls Podman, runs commands, publishes DNS/IP, grants permits, performs
fencing or activation, or changes enrollment.

## Resident behavior

A persistent TCP listener admits a fixed number of workers. Each calls the
transport's `Node::serve_connection_reporting` for exactly one authenticated, bounded
frame and atomic import. Excess connections are closed and counted without an application
queue; the kernel has its own finite backlog. One outgoing worker visits the exact
configured peers sequentially, with independent bounded exponential backoff.
Successful exchange resets the peer interval. An unreachable peer does not become
a dead host or an activation decision. A slow peer delays others by its bounded
attempt; hostile admission fairness is not guaranteed.

Operation IDs and nonces use OS randomness. The same observed snapshot reuses an
operation ID, avoiding a new receipt for each unchanged poll. After restart, IDs
are fresh. The outgoing worker records the digest of the snapshot each peer last
acknowledged with an authenticated receipt: it exchanges again when the local snapshot
differs, and otherwise only after the unchanged-snapshot refresh delay, ten minutes by
default. An idle replica therefore adds its audit rows once per refresh, not at every
interval: at three replicas, two pushes sent at two rows each and two received at four,
twelve rows per refresh or at most 1,728 a day. The acknowledgement state restarts
empty, so a restarted process pushes to every peer at once.

Push-back covers the other direction. When an authenticated import from a peer commits
and leaves this replica with more facts than that peer's snapshot carried, the replica
forgets that peer's acknowledgement and makes its next push to it due at once, at most
once per interval per peer. A replayed receipt reports the counts of its original
commit and pushes nothing back. An attempt that overlapped such a request does not
record its acknowledgement, so the requested push follows it. A peer restarted on an
emptied or older store therefore receives the facts it lacks within a few exchanges of
its own first push, not after the refresh. The transport exports again inside its
call: an incoming import between the scheduler's snapshot digest and that export can
cause a transient replay binding refusal. A changed digest creates a fresh ID on the
next attempt. The transaction refuses mismatches rather than making an unsafe mutation.

A process appends no local fact before it has caught up with its peers. A store that
lacks some facts of its own origin, deleted or restored from an older copy, would
otherwise number its next fact with a producer sequence its peers already hold with
other bytes, and each side would refuse every later import from the other, for good.
A peer is caught up with once this process has committed or replayed an authenticated
import from it that carried every fact the peer was known to hold, or once that peer's
latest authenticated receipt for a push of this process counted exactly the pushed facts:
having imported them, the peer held nothing this replica lacked. Without that second
rule a clean restart would wait for the window, since its peers have nothing to push
back and still hold an acknowledgement of their own snapshot. A receipt counting more
facts than were pushed marks the peer ahead until an import from it carries at least as
many. A refused import or push counts for nothing. Every peer caught up with catches the
process up. The catch-up window forgives the other peers only to a store whose latest
fact of its own origin was appended by the store itself (its `observe` receipts say so;
an emptied store that imported its own facts back never qualifies, in any later
process), once the window has elapsed since the process started exchanging, one peer has
been caught up with, and every other peer has been attempted and is not known to be
ahead. This bounds a start while a peer is down, at the price of the collision above if
that peer alone held later facts of a restored older copy. A replica that reaches no peer
never appends. A replica without peers is caught up at once. Until it is caught up,
`append_observation` answers `append_observation_catching_up` without touching the store.
The state latches for the process.

A refused authenticated import is reported too. After a refused import the transport
compares the refused snapshot with the local history; the resident counts refused
imports per peer and, when the snapshot carried an event ID this replica holds with
other bytes, an identity collision, named once on standard error for each new colliding
event ID from that peer. A push refused with `policy_violation` counts as a collision
once one was found in that peer's own pushes.

SQLite retains the dependency's WAL/FULL, immutable history and receipt checks,
identity binding and atomic import. The resident's first store open verifies the whole
store before the listener and control socket exist; a background worker repeats that
complete verification, in a read transaction, every full-verification interval, and
writes a failed pass to standard error. A pass that could not run, because the store did
not open or the pass could not take its snapshot, verified and closed nothing: it is
retried after a backoff that starts at `interval_ms` and doubles up to
`max_backoff_ms`, never later than the verification interval, and each attempt is
written to standard error. A failure that closed the store is told apart by the store's
integrity state: later passes keep the verification interval and write that the store is
still closed. The main loop supervises that worker like the outgoing one: a resident
whose verification or replication worker stopped exits with an error. Any failure to
read or verify a stored row, in a pass or in any transaction, closes that database file
for the process, and no later transaction of the process commits on it: appends answer
`append_observation_uncertain`, exchanges fail, status and shutdown stay available, and
a restarted resident refuses to start on that store. The closed state belongs to the
database file, not to its path: a different file placed at the configured path is
verified completely at its next open by the process before it is served. A held lock
file prevents two residents on the same configured database path. It does not fence
copied databases, path aliases or a privileged actor. No history/receipt compaction or
disk quota exists yet.

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
`incoming_workers`. `observation_writer_uid` fails closed when absent. Three fields
are optional and omitted when serialized unset: `full_verification_interval_ms`
(default 600,000), `unchanged_snapshot_refresh_ms` (default 600,000) and
`catch_up_window_ms` (default 15,000). The
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
| Unchanged snapshot refresh | Optional, at least interval, at most 3,600,000 ms; default 600,000 ms |
| Catch-up window | Optional, 1,000–15,000 ms; default 15,000 ms: the universe's 25-second start budget, counted in whole seconds, holds while the control socket binds within about 8 s |
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
a prior request with that operation ID will commit. A process that has not caught up
with its peers returns `append_observation_catching_up` after the authorization checks
and before any store access; retry the identical request. `status_unavailable` is a
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
`canonical_inspection_available_via: "--inspect-store"`. It also carries
`store_closed` and `store_closed_reason`: whether the store is closed for this
process and the failure that closed it, read from the store's closed state rather
than from the store itself, so status stays available while the store serves
operations elsewhere. It is what fails closed with the store: the administration
app's origin refuses everything under `/admin` with `503` while `store_closed` is
true, and refuses it too when this status does not arrive. Full canonical facts,
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
`history_count_delta`. Each peer also reports `last_attempt_age_ms`, the age of the
end of the last attempt whatever its outcome (equal to `last_success_age_ms` when that
attempt succeeded, smaller once one failed after it); `acknowledged_unchanged`, true
when that peer's receipt acknowledges the current local snapshot, the refresh has not
elapsed and no push is due; the effective `refresh_ms` and `max_backoff_ms`; and
`push_backs`; `authenticated_refusals` and `last_refusal_reason` for its pushes;
`refused_imports`; and `identity_collisions` (`imports_refused`, `pushes_refused`,
`event_id`). An idle link is confirmed once per refresh. A live link's last success is
never older than the refresh plus one interval plus one exchange with each peer, since
the outgoing worker visits them in turn (an exchange is bounded by connect, write and
read deadlines of 2 seconds each); a link whose attempt failed is retried within the
maximum backoff. The liveness bound a consumer should apply is therefore the refresh
plus the maximum backoff plus one exchange, with a margin for those visits, and a failed
attempt after the last success speaks at once. A peer that stops answering on an idle
link is reported by the first attempt after it, at most one refresh plus that same
margin after the last success.
`catch_up` reports `caught_up`, `caught_up_by` (`every_peer`, `window` or `no_peers`),
`caught_up_after_ms` since the process started exchanging, `peers_imported`,
`peers_matched`, `peers_missing`, `peers_ahead`, `peers_not_attempted`,
`own_facts_at_start`, `latest_own_fact_appended_locally` and `window_ms`. None of this is
exact causal lag or convergence proof: equal counts can differ, and replayed receipts describe a
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
