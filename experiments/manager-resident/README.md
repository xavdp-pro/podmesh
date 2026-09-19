# Resident control-services replication laboratory

Status: local executable laboratory, no deployment or HA claim. One logical
manager retains control facts in distinct replicas, without a shared live
filesystem. This increment runs periodic authenticated snapshot exchange using
the typed `manager-network` and durable SQLite `manager-ha` path dependencies.
It never calls Podman, runs commands, publishes DNS/IP, grants permits, performs
fencing or activation, or changes enrollment. Configured to, it signs votes under a
signing ledger ([below](#signed-votes-and-the-signing-ledger-v3-4)); a vote grants
nothing until k of them make a certificate that a PodMesh node verifies itself. Configured to
decide, it reads the proposals its peers carry, checks each against its own view and votes for the
ones that pass ([the manager decides](#the-manager-decides-v3-5)).

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
ahead — all of it on evidence no older than the window's end, which is why every peer is
made due again when the window ends, with a fresh operation ID, its acknowledgement
forgotten and its backoff restarted. A peer caught up with earlier may since have
received, relayed from the peer the window would forgive, the very facts this store
lacks. Receipts date that evidence (a receipt counts the peer's history no earlier than
the attempt that drew it, or than the operation ID behind a replayed one); an imported
snapshot does not, so it counts through the receipt whose history it covers. A peer that
answers an authenticated refusal is reached, not forgiven. This bounds a start while a
peer is down, at the price of the collision above if that peer alone held later facts of
a restored older copy. A replica that reaches no peer never appends. A replica without
peers is caught up at once. Until it is caught up, `append_observation` answers
`append_observation_catching_up` without touching the store, and the replica keeps
exchanging meanwhile: it is running, and not ready. The state latches for the process.
While it is catching up, an authenticated import from a peer also makes its own next
attempt to that peer due at once: the process needs a receipt of its own, which no
import gives.

A refused authenticated import is reported too. After a refused import the transport
compares the refused snapshot with the local history; the resident counts refused
imports per peer and, when the snapshot carried an event ID this replica holds with
other bytes, an identity collision, named on standard error the first time that event
ID is seen from that peer and never again for it, in whatever order and however often
the refusals repeat. A push refused with `policy_violation` counts as a collision once
one was found in that peer's own pushes; it names nothing by itself, since a signed
refusal carries only its reason.

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
| Catch-up window | Optional, 1,000–15,000 ms; default 15,000 ms, plus the round of attempts taken after its end (one connect deadline per unreachable peer). The universe's 25-second start budget does not bound it: a replica catching up is running, not ready |
| Control request frame | 32,768 bytes within the shared 250 ms control deadline |
| Observation value | Nonempty UTF-8, at most 4,096 bytes |
| Control response | 32,768 bytes within the shared 250 ms control deadline |
| Vote operation (V3-4, V3-5) | Answered within 2,000 ms of its request; the control loop keeps serving meanwhile |
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
and before any store access; retry the identical request, for as long as it takes. It is
not a refusal: the replica is running and exchanging, and it appends nothing before it
holds every fact of its own its peers hold. `status_unavailable` is a
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
listener, socket, worker, interval or peer-key validation. Run it on a stopped
replica's store, or on the live one through this command, which captures the file by
reading it twice and keeps the capture only when both reads agree: never on a
`podman cp` of a live store, whose database and WAL are copied one after the other
while the resident commits and which is torn (missing rows, rowid gaps, a malformed
image). Its output carries every
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
`peers_matched`, `peers_missing`, `peers_ahead`, `peers_not_attempted` (neither caught
up with nor attempted),
`own_facts_at_start`, `latest_own_fact_appended_locally`, `window_ms`,
`appends_observed` (the appends this process answered observed) and `blocked_by`, why
it is not ready: `waiting_for_peers`, `refused_by_peer`, or `identity_collision` with
the peer and the colliding event ID, which waiting never resolves. None of this is
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

## Signed votes and the signing ledger (V3-4)

A replica can sign a **vote**: its promise for one exclusive decision that a PodMesh node will
verify as a quorum certificate of V3-2 (`podmesh-takeover-proof/quorum-ed25519` for an epoch
rotation or a same-holder re-issue, `podmesh-policy-change/quorum-ed25519` for a change of the
authority set). k votes of distinct keys on one payload assemble into exactly the certificate the
node accepts. A vote grants nothing by itself, and `activation_authority` stays false: the node
verifies the certificate, and proposing, deciding and delivering one are the next lot's (V3-5).

**Custody.** The replica's key and its signing ledger live in a directory the host provides outside
the universe's state, named by `PODMESH_MANAGER_VOTE_DIR` (private, of the service's user):
`<key_id>.key` holds the 32-byte seed as 64 lowercase hex characters, and `<key_id>.ledger` is the
ledger. No recovery point, restore, clone or migration of the replica's universe carries either.
The host also mounts its own machine-id read-only at `PODMESH_MANAGER_HOST_ID_FILE` (default
`/etc/machine-id`), not inside the vote directory: a universe's own `/etc/machine-id` travels with
a clone, and a copy of the vote directory must not carry the identity it is checked against. The
resident checks it at the start and at every operation: an absolute path to a regular file, opened
without following a symlink, on a mount that is read-only for the process (`statvfs`
`ST_RDONLY`). Otherwise it signs nothing and answers `host_identity_not_read_only` (or
`host_identity_unreadable`); at the start it also writes `resident vote alert:` and the code on
standard error. This keeps a replica from rewriting the identity its ledger is bound to. It does
not tell a whole-VM clone from its original, which carries the same machine-id, key and ledger:
the operator's rule of no VM clone of a laboratory host on the managed network (A3) stays what
closes that. A configuration with `votes` and no vote directory does not start.

**The ledger** (rule R4 of the quorum model: the promise lives with the key). Per resource, one
entry per epoch with its holder, and one entry per `from_serial` with the new policy's digest.
Neither is keyed on the policy digest, so a change of the authority set never reopens an epoch or
a serial. The key signs one decision per (resource, epoch) and one per (resource, from_serial): the
same decision again, with a fresh life (`issued_at`, `expires_at`) only, is re-signed; any other
decision for a promised number, or for a number below one promised, is refused. Every signature
takes the next sequence number. The header binds the key (its id and public half), the host's
machine-id and a random nonce made with the ledger; a checksum covers the file. A signature is one
step under an exclusive `flock` on the vote directory itself, opened without following a symlink
and checked after locking to still be the directory its path names (a lock file could be removed or
replaced under a live signer, and the next signer would lock a new file): check, write the new ledger to a
temporary file, fsync it, rename it over the ledger, fsync the directory, and only then sign. A
missing, unreadable, foreign (another key or another host) or unadmitted ledger signs nothing and
answers a named code: `ledger_missing`, `ledger_unreadable`, `ledger_foreign_key`,
`ledger_foreign_host`, `ledger_unadmitted`.

**The vote.** `form` `podmesh-manager-vote/1`, the `voter` (key id), the `certificate_kind`, the
`payload` (the certificate without `signatures`), `signature` (the voter's Ed25519 signature over the
payload's canonical form: the entry the node counts), the ledger's `ledger_nonce` and
`ledger_sequence`, and `envelope_signature`, the voter's signature over all of that. A vote counts
only when its voter is a key of the policy (`unknown_voter` otherwise), both signatures verify
strictly under that key (`bad_envelope_signature`, `bad_signature`), its payload names this policy
(`policy_mismatch`), and, read from a fact, the fact's origin replica owns that key
(`origin_mismatch`). A replica signs only a payload live on its clock, issued at most 30 seconds
ahead, living at most `max_certificate_life_seconds`, under the policy it is configured with, and a
policy change only away from its own serial.

**The tripwire.** Before signing, the resident reads every vote its store holds (its own and its
peers'). A vote of its own key, whose signatures verify, numbered above the ledger
(`ledger_behind_own_votes`), or numbered since the ledger's last admission and not in it
(`own_vote_unknown_to_ledger`), shows the ledger went back in time: a restore or a snapshot revert.
The resident then marks the ledger unadmitted, durably, refuses, writes `resident vote alert:` and
the code on standard error, and keeps the alert in its status. A forged vote cannot fire it.

**Readmission** is the operator's, and fails closed. It reads this replica's store; for every other
replica its store, and for every other key of the policy its ledger, and for every node of
`votes.nodes` its screen, from evidence files the operator places in `votes.evidence_dir`
(`store.<replica_id>.json`, `ledger.<key_id>.json`, `screen.<host_uuid>.json`, each
`{"form": "podmesh-manager-readmission-evidence/1", "input", "source", "collected_at", "content"}`,
the content being an `--inspect-store --facts-only` output, a ledger file, or
`{"activation_status": [...]}`); and the ledgers of this host's retired keys still in the vote
directory. The evidence directory is outside the vote directory (`evidence_dir_inside_vote_dir`)
and one the replica cannot write, now or after a `chmod` of its own: owned by the operator and not
writable by the service's user, or mounted read-only into the universe (`evidence_dir_writable`
otherwise, naming the directory or file). Nothing is read from the vote directory but this host's
own ledgers. The request states the SHA-256 of every evidence file (`evidence_sha256`, file name to
digest); each file is read once, hashed, and parsed from those bytes, and a digest that is not the
one stated, or a digest stated for a file readmission does not read, refuses by name
(`readmission_evidence_mismatch`); a required file with no digest stated is unreadable. The
replica can therefore not substitute what the operator collected. It refuses, naming each input (`readmission_inputs_unreadable`), when one is missing,
does not parse, is not what it claims (a store whose digest does not match its facts, a ledger whose
checksum fails, a screen of another host), was collected before the ledger was marked, or holds a
vote of a key it does not know (a retired key must be named in `votes.retired_keys`). It never
proceeds on what it could read. It then waits until `max_certificate_life_seconds` plus 60 seconds
have passed since the ledger was marked (`readmission_too_early`, with `retry_at`), so that a
signature the ledger forgot and a proposer still holds has expired on every clock: 60 seconds is the
bound on the skew between any two clocks, the node's 30-second allowance on each side. It sets, per resource, the
epoch floor at the highest epoch anything showed (a promise, a vote, a screen) and the serial floor
at the highest `from_serial` promised or already passed, raises the sequence above the key's own
votes, records what it read (with digests) and what it set, and admits the ledger. Where an input
cannot be read, the operator waits for it: nothing replaces reading it. Re-keying is the way out for
a ledger that cannot itself be readmitted (lost, unreadable, or bound to another host): a new key and
ledger for the replica (an authority-set change at serial + 1), admitted by the same operation from
the same inputs, which reads the old key's votes as a retired key's, so its floors are above them
too. The node screens themselves move when the first certificate above them is delivered (V3-5);
they never move backwards.

**Control operations.** Each one JSON object with an `operation_id` token:

```json
{"operation":"vote_ledger_init","operation_id":"init-1"}
{"operation":"vote_ledger_mark_unadmitted","operation_id":"mark-1","reason":"host restored from a snapshot"}
{"operation":"vote_ledger_readmit","operation_id":"readmit-1","evidence_sha256":{"store.r1.json":"<sha256>","ledger.replica-b.json":"<sha256>","screen.<host_uuid>.json":"<sha256>"}}
{"operation":"vote_sign","operation_id":"sign-1","payload":{"kind":"podmesh-takeover-proof/quorum-ed25519","...":"..."}}
```

The first three require the caller's UID to be `votes.operator_uid`; `vote_sign` requires
`observation_writer_uid` and a caught-up process (`vote_catching_up`). Another caller gets
`vote_operation_refused` with `caller_uid_refused`. `vote_ledger_init` creates an unadmitted ledger
and refuses an existing one (`ledger_exists`). `vote_sign` records the vote as a fact in
`votes/<replica_id>` (subject `epoch:<resource>:<epoch>` or `serial:<resource>:<from_serial>`)
before it answers `{"vote", "fact", "replayed"}`; a vote that could not be recorded is not answered
(`vote_unrecorded`), and its promise stays in the ledger. The operations share the append worker:
`vote_busy` while it is occupied, `vote_operation_uncertain` when the worker did not finish inside
the vote deadline (the ledger and the store keep what it did; `status` shows the ledger). **A
signature is idempotent (V3-5):** a payload this replica already voted for, whose promise its ledger
holds, is answered with the vote already recorded (`replayed: true`), whatever the `operation_id`,
nothing signed or appended again, even once its certificate has formed. **The vote deadline
(V3-5):** a vote operation reads the store, verifies every vote and writes durably, which can
outlast the 250 ms control deadline on a busy disk (342 ms measured once, with three residents on one
workstation disk and a debug build). The control loop no longer waits for it: it keeps serving
status, appends and the network, and answers the vote operation when its worker finishes, within
2,000 ms of the request, or `vote_operation_uncertain` after that. `status` reports each operation's
last and longest duration (`votes.operations`). Refusals are `{"error": "vote_refused", "code",
"detail"}` and `{"error": "readmission_refused", "code", "detail", "unreadable", "retry_at",
"alternative"}`. `status` carries `votes`: `key_id`, `ledger_state` (`admitted`, `unadmitted` or the
refusal code), `sequence`, `unadmitted_since`, `unadmitted_reason`, `admissions` and `alerts`.

The optional `votes` configuration: `key_id`, `authority_id`, `authority_quorum` (the nodes'
`{"threshold", "keys"}`), `authority_serial`, `replica_keys` (every replica of the topology and its
own key of the policy), `retired_keys` (`[{"key_id", "public_key"}]`, optional), `nodes` (1 to 64
host UUIDs), `evidence_dir` (absolute), `operator_uid` and `max_certificate_life_seconds` (1 to 3,600). The topology must grant
`votes/<replica_id>` to this replica.

**The restore checklist.** The tripwire sees only the key's votes that reached this replica's
store; a restored ledger shown none of its later votes signs again what it forgot (the
`host_restore_silent` cell of the quorum model, 323 runs in 2,000). The mark is therefore the
precondition of every restore, not a courtesy. A conservative kernel-boot guard now runs before a
voting resident exchanges with peers and again before every ledger operation. It keeps a private,
durable `<key_id>.boot-id` beside the ledger. If that witness is absent or differs from the current
kernel boot ID, it durably marks an admitted ledger unadmitted **before** advancing the witness. A
failed read or write prevents voting. This covers file-based VM backup restores that start a fresh
kernel, even when the restored store contains no later vote to trip the ordinary check. It also
marks on an ordinary reboot: the local files cannot prove that the reboot was not a restore, so
operator readmission is required again. A process restart within the same boot retains admission.

For a VM host that exposes a generation identifier outside the guest snapshot, set
`votes.require_generation_id` and mount that live identifier read-only at the path in
`PODMESH_MANAGER_GENERATION_ID_FILE`. The resident accepts either a 16-byte identifier or QEMU's
4096-byte `etc/vmgenid_guid` fw_cfg item (the 16-byte value at offset 40). It keeps a separate
private `<key_id>.generation-id` marker and checks it before every ledger operation. Missing,
unreadable or changed evidence refuses voting; a changed value marks the ledger unadmitted before
the marker advances. The med-pmox campaign requires this mode. The guest boot guard remains in
place as a second signal. A memory snapshot resumed in the middle of an already running signing
operation has not yet been adversarially qualified, so this does not alone authorize that restore
mode for a voting VM.

Without this hypervisor witness, the guard cannot detect a snapshot resumed with its old kernel
memory, or an in-place rollback of the vote directory during the same boot. Those require an
external restore witness or explicit isolation and marking. After any revert of a host VM's snapshot, any restore of a host from a
backup, or any restore of the vote directory:

1. Keep the host's agent and vote callers stopped. On a fresh kernel boot, verify that every
   ledger is unadmitted by the boot guard before enabling vote callers. For an in-place rollback
   on the same boot, run `vote_ledger_mark_unadmitted` for every key before catch-up or voting;
   only then start the agent.
2. Collect the evidence from every other host into the evidence directory, after the mark, and
   record each file's SHA-256.
3. Run `vote_ledger_readmit` with those digests once `retry_at` has passed. If an input cannot be
   read, wait for it.

No guest-local marker can distinguish a legitimate reboot from a restore. Everything on the host, a
marker file, the ledger's mtime, its nonce, the store, is reverted with the host's snapshot; the
boot ID changes at every legitimate reboot too. The new guard therefore chooses safety over
automatic readmission. Only what lives off the guest can distinguish a restore: the other hosts,
which the tripwire reads, the operator, who marks, and a hypervisor generation ID when mounted.

**Not here** (see [the manager decides](#the-manager-decides-v3-5) for what V3-5 added): lease
extension by majority (V3-6); re-keying without a trusted dealer.
Clones of a whole host VM (key, ledger and machine-id together) are indistinguishable from the
original: the operator's rule of no VM clone of a laboratory host on the managed network stands.

## The manager decides (V3-5)

With `votes.decisions` configured, the replicas decide an epoch rotation or a same-holder re-issue of
the resources it names, by majority, and nothing else: declaring a host lost stays the operator's
recorded decision, and a lease is still extended by its holder (V3-6).

**A proposal** is a fact. It carries the full payload of a takeover certificate: the resource, the
target epoch, the new holder and its `holder_boot_id`, the grant, the method, the barrier
`eligible_after`, and the life `issued_at` and `expires_at`. It is recorded by
`decision_propose` in the proposing replica's scope `proposals/<replica_id>` (the topology must grant
it; `proposals_not_granted` otherwise), and replication carries it:

```json
{"operation":"decision_propose","operation_id":"propose-1","payload":{"kind":"podmesh-takeover-proof/quorum-ed25519","...":"..."}}
```

The proposing replica checks the payload's shape, policy, resource and life only
(`proposal_refused` with a code otherwise), and answers `{"proposal", "payload_digest", "here"}`,
`here` being its own verdict. **The proposer is not trusted.**

**The voter** is a worker of the resident. Every `voter_interval_ms`, once the process has caught up
with its peers, it reads the proposals its store holds. It checks each live one above its view
(`decisions.rs`, `check`) and votes, under the V3-4 signing ledger, for one that passes. The vote is
recorded in `votes/<replica_id>`. The rules, each refusal named:

- a takeover certificate of this replica's policy (`decision_kind`, `policy_mismatch`), for a resource
  it decides (`resource_not_decided_here`), carrying no field the node does not bind
  (`payload_unknown_field`), live on this clock and living at most `max_certificate_life_seconds`
  (`certificate_life`);
- its holder a node of `votes.nodes` (`holder_not_a_node`), its identifiers ones the node accepts
  (`payload_invalid`), its barrier no later than its expiry (`barrier_after_expiry`);
- **the view**: the highest-epoch certificate the store's votes assemble into, counted only from each
  voter's own scope and only on verified signatures, or before any, the operator's recorded
  `baseline` for a resource moving from the gate. The proposal is the next epoch (`epoch_not_next`)
  with the view's holder as its previous holder (`previous_holder_mismatch`). A view that holds two
  certified decisions for one epoch decides nothing (`conflict_in_view`);
- **the method** (`MANAGER-PUBLISHER-CONTRACT.md` of the node, "The barrier, as it is"):
  - `first` only before any epoch (`first_after_an_epoch`);
  - `same_holder` only to the current holder (`not_the_same_holder`), carrying its barrier;
  - `lease_barrier` carrying the barrier and, when the holder changes, covering every way the previous
    holder may still hold its lease without being told, each plus the lease and the margin: its
    re-acquisition under the current certificate until that expires, a renewal by itself until
    `renewal_not_after` (the follow mandates' `not_after`, the operator's recorded bound), and an
    acquisition at the proposal's issue (`barrier_too_early`, naming the barrier required). Until the
    majority extends leases (V3-6), a change of holder is refused while `renewal_not_after` is 0
    (`renewal_unbounded`) or earlier than the current proof's expiry or the proposal's issue
    (`renewal_bound_too_early`), and, from a baseline, while the baseline does not name the gate's last
    proof's `expires_at` (`baseline_expiry_unknown`);
  - `fence_receipt` is refused (`fence_receipt_unverifiable`): a receipt is the previous holder node's
    unsigned answer to its own fence, which no replica can verify, and one that shortened the barrier
    would let a single proposer start a second holder.

The ledger then keeps the promise of one decision per epoch. A voter whose store lags sees a lower
epoch and votes a round later: a late vote costs time, never safety. `vote_sign` applies the same rules to a
takeover payload and refuses a policy change (`decision_kind`), when `votes.decisions` is configured.
Without it, V3-4's behaviour is unchanged.

**The read.** `decision_read` (the observation writer or the operator) answers:

```json
{"operation":"decision_read","resource":"<uuid>"}
{"decision":{"resource","view":{"epoch","holder","barrier","expires_at","source"},
             "current":{"epoch","holder","payload_digest","voters","certificate"}|null,
             "pending":[{"payload_digest","new_epoch","new_holder","method","eligible_after","expires_at","live",
                         "proposed_by","voters","votes","threshold","missing","here"}],
             "conflicts":[],"threshold","keys","read_at"}}
```

`current.certificate` is what a PodMesh node verifies: k signatures of distinct keys over one payload,
checked with the node's rules before it leaves. `pending` lists up to eight proposals above it, each
with what is missing and this replica's own verdict (`here`: `voted`, `refused` with its code,
`expired`, or `not_evaluated`). A host reads it through the node's door (`manager_decision`) and
delivers the certificate with its decision follow tick (the node's `packaging/podmesh-decision-follow`).

**Configuration.** `votes.decisions`:

```json
{"voter_interval_ms": 1000,
 "resources": [{"resource": "<uuid>", "lease_seconds": 300, "takeover_margin_seconds": 30, "renewal_not_after": 0,
                "baseline": {"epoch": 157, "holder": "<host uuid>", "eligible_after": 1789681724}}]}
```

`lease_seconds` and `takeover_margin_seconds` are the nodes' policy for the resource: the longest lease
any holder holds. `renewal_not_after` is the latest second any holder may renew by itself. `baseline` is
optional; its `expires_at` is the gate's last proof's expiry, which the view uses as the current expiry
until a certificate the replicas assembled passes the baseline.

**Before any rotation to another holder (review of V3-5, findings 1 and 2).** The replicas cannot see a
follow mandate, and the node does not re-check one. Until V3-6, the operator freezes (no new `--refresh`)
or removes the V3-1 follow mandates on every host, then sets every replica's `renewal_not_after` to the
latest `not_after` among them, and restarts the replicas with it. A resource leaving the gate gets the
gate's last proof's `expires_at` in its baseline. The voters refuse a change of holder otherwise. The topology grants `proposals/<replica_id>` to each replica that proposes.
`packaging/podmesh-manager/universe/replicated/add-votes.py` writes all of it for a generated replica set.

**Readmission evidence** is gathered by `tools/collect-readmission-evidence.py`. From a plan naming one
command per input, it collects:

- every other replica's store (`--inspect-store --facts-only`);
- every other key's ledger;
- every node's screen (its `screen` subcommand, run on that node's host).

The ledger is marked first: the plan names the replica's control socket, and the tool refuses a ledger
not marked unadmitted. Each input is read twice, `--settle-seconds` apart, and refused if it changed;
each screen carries the node's clock (`observed_at`), which must not be older than the mark. It writes
the inputs read-only into the operator's evidence directory in the form readmission reads, and prints
the `evidence_sha256` map and the request to send. It fails closed and writes nothing when an input is
missing, extra, fails, is not what it claims, changed between its reads, or is a screen older than the
mark. **The digests are not proof that the cluster was live**: they name the bytes the operator vouches
for. A command that returns a consistent older copy of a store or a ledger passes every check but the
screens' clock, and the operator vouches that each command reads the live input of the host it names.

## Validation and gaps

[EVIDENCE.md](EVIDENCE.md) covers three simultaneous compiled processes, UID-bound
nonexclusive observations, live TCP partition/reconnect, SIGKILL/restart,
conflicts, wrong keys, authenticated invalid batches, raw-frame/value bounds,
read-only inspection, framing/admission limits and shutdown. The partition test
uses six TCP proxies to cut both directions around one still-running resident.

Remaining: key rotation/revocation, signed original provenance of facts other than votes, encryption,
WireGuard/real-host qualification, bounded incremental history, retention/quota,
receipt/backup recovery, identity fencing across paths/hosts, Logger, integrated
control-services deployment and actual exclusive-effect fencing. There is no
automatic HA, quorum-free takeover, zero-loss or production availability claim.

```sh
cargo test --locked --manifest-path experiments/manager-resident/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-resident/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-resident/Cargo.toml -- --check
```
