# G2 durable authenticated exchange implementation contract

Status: implementation-ready laboratory contract. It defines the smallest
increment that can qualify the G2 authenticated-exchange gate on three installed
laboratory hosts. It is not implementation evidence and does not qualify manager
high availability.

Owner: Xavier de Poorter, collaborating with OpenAI Codex. Independent
counter-review is performed by Claude Code and every finding is verified by
Codex before it changes the implementation.

## Objective

Qualify one logical manager with three distinct host-bound replicas for these
claims only:

1. an authorized local caller can append an immutable non-exclusive observation
   through the resident Unix control interface;
2. configured peers exchange those observations through the bounded mutually
   authenticated transport;
3. accepted and refused exchanges leave durable, integrity-checked evidence;
4. an external process can inspect each canonical store through a supported
   read-only interface without direct SQL or store mutation; and
5. three installed replicas converge, survive a clean process restart and catch
   up after a bounded network partition.

The implementation remains a laboratory increment because the current transport
uses plaintext TCP with pairwise HMAC keys embedded in protected configuration.
It has no production credential rotation, revocation or payload confidentiality.
The three-host trial must use disposable manager state and non-production facts.

## Relationship to the target manager

The product target is one logical manager universe, replicated once on every
participating host. In the preferred deployment that universe runs inside
ShaperOS and carries the registry, internal naming service and the tools used by
the Governor and Makers. Standalone Linux remains supported, so none of the
replication mechanics may depend on ShaperOS.

This design replaces a cluster-wide shared configuration filesystem with local
durable stores and explicit authenticated exchange. A host can retain its local
facts while disconnected and reconcile them after connectivity returns. The
design therefore avoids making ordinary local operation depend on the health of
one shared filesystem or one global configuration mount.

G2 proves only the exchange substrate for that target. It does not yet prove
that the replicated manager can take over safely. Effect authority, stale-leader
rejection and partition reconciliation remain separate gates because multiple
copies of the same logical manager may be alive while connectivity is divided.

## Non-goals and authority boundary

This increment must not add or imply:

- Podman lifecycle operations, checkpointing or migration;
- DNS or route publication;
- service-IP activation;
- failure detection, fencing, takeover or coordinator effects;
- dynamic peer enrollment, discovery or trust on first use;
- configuration, topology, path or grant replication;
- an exclusive fact through the local control interface;
- a quorum or a high-availability claim;
- production key lifecycle or transport confidentiality; or
- ShaperOS integration.

`activation_authority` remains exactly `false`. A receipt proves a bounded local
commit or an authenticated peer reply. It is never an effect permit. An
unreachable peer remains unknown rather than stopped.

## Required invariants

| ID | Invariant |
| --- | --- |
| G2-I01 | The local append interface can create only `exclusive_resource: null`, `active_claim: false` facts. |
| G2-I02 | The configured Unix peer UID and the predeclared scope owner must both authorize a local append. |
| G2-I03 | Facts, mutation receipts and accepted inbound-import audit evidence commit atomically. |
| G2-I04 | A successful remote result is accepted only after its peer HMAC, request binding and durable receipt evidence verify. |
| G2-I05 | A refusal is called authenticated only when the request was authenticated first and the refusal reply verifies. |
| G2-I06 | Pre-authentication diagnostics never become peer authority or a verified refusal. |
| G2-I07 | Every completed exchange has bounded durable evidence; a prepared attempt with no terminal event remains explicitly incomplete. |
| G2-I08 | Byte counts are actual framed bytes transferred in that phase, including the four-byte length prefix; request and reply announced sizes are recorded separately. |
| G2-I09 | Audit data contains no pair key, configuration bytes or unbounded attacker-supplied detail. |
| G2-I10 | Canonical inspection captures stable database, WAL and SHM bytes, opens only the private copy read-only, and never initializes, migrates, checkpoints or repairs the canonical store. |
| G2-I11 | Corrupt facts, receipts or audit rows fail closed before replay, mutation or a successful inspection result. *Amended 2026-09-17 (lots V2-R and V2-S):* a row added since the last verification without removing a stored row, a replayed receipt or audit row, the rows of the attempt an audit row extends and the facts an operation loads fail closed before that replay, mutation or reply; every row fails closed at a process's first open, at each periodic complete verification and before a successful inspection result; a failure closes the database file for the rest of the process, which commits no transaction on it afterwards. See [MANAGER-PRE-REPLY-VERIFICATION.md](MANAGER-PRE-REPLY-VERIFICATION.md). |
| G2-I12 | Retrying an identical operation reuses its mutation result but records a distinct transport attempt. |
| G2-I13 | Reusing an operation ID for different request content refuses without partial durable state. |
| G2-I14 | Audit or receipt failure prevents a success claim even when the remote outcome is uncertain. |

## Durable schema v3

`experiments/manager-ha/src/durable.rs` owns the schema and every database
mutation. The network and resident crates must not execute SQL directly.

Schema v3 retains the immutable `identity`, `facts` and `receipts` tables and adds
an immutable `exchange_audit_events` table. Existing schema versions remain
refused without mutation. This laboratory has no automatic v2-to-v3 migration.
The installed replicas have not yet created an active manager database, so their
first qualified activation must create a fresh v3 store.

The new table has these logical fields:

| Field | Contract |
| --- | --- |
| `audit_event_id` | Unique locally generated ASCII token; primary key. |
| `attempt_id` | Unique locally generated `attempt:<64 lowercase hexadecimal SHA-256>` identity for one exchange attempt. It is never copied from the peer. |
| `wire_nonce` | A validated decoded peer protocol nonce, or a locally generated `preauth:<64 lowercase hexadecimal SHA-256>` connection nonce before decoding reaches one. It remains separate from local attempt identity. Reuse cannot close another attempt. |
| `direction` | `inbound` or `outbound`. |
| `phase` | Typed phase from the fixed list below. |
| `authenticated_peer_id` | Configured authenticated peer, otherwise null. |
| `peer_claim` | Untrusted claimed peer identity when decoding reached that field, otherwise null. |
| `operation_id` | Authenticated wire operation ID when available, otherwise null. It may differ from either receipt identity. |
| `request_frame_bytes` | Actual request bytes read or written in this phase, including the length prefix; never a cumulative attempt total. |
| `request_announced_body_bytes` | Declared body size when the prefix was available. |
| `request_sha256` | Digest of complete request body when available, otherwise null. |
| `reply_frame_bytes` | Actual reply bytes read or written in this phase, including the length prefix; never a cumulative attempt total. |
| `reply_announced_body_bytes` | Declared reply body size when known from a prepared body or received prefix. |
| `reply_sha256` | Digest of complete reply body when available, otherwise null. |
| `outcome` | Typed outcome from the fixed list below. |
| `error_category` | `unavailable`, `refused` or `malformed`, otherwise null. |
| `reason_code` | Closed `RefusalReason` enum; never raw remote or operating-system text. |
| `local_receipt_operation_id` | Destination-local durable mutation receipt reference, otherwise null. For authenticated imports it is a bounded source-namespaced digest identity. |
| `local_receipt_sha256` | Verified local receipt checksum, otherwise null. |
| `remote_receipt_operation_id` | Receipt identity conveyed by an authenticated peer for an accepted outbound completion, otherwise null. |
| `remote_receipt_sha256` | Receipt checksum conveyed and HMAC-bound by the peer, otherwise null. |
| `replayed` | Whether the referenced durable mutation result was replayed. |
| `record_json` | Canonical typed serialization of every field above. |
| `sha256` | Domain-separated checksum of `record_json`. |

Allowed phases are:

- `outbound_request_prepared`;
- `outbound_exchange_completed`;
- `inbound_request_observed`;
- `inbound_import_committed`;
- `inbound_refusal_recorded`;
- `inbound_reply_prepared`;
- `inbound_reply_write_observed`;
- `inbound_diagnostic_reply_written`; and
- `inbound_connection_closed`.

Allowed outcomes are:

- `accepted`;
- `authenticated_refusal`;
- `unauthenticated_diagnostic`;
- `unavailable`;
- `malformed`; and
- `incomplete`, stored only on a prepared phase. Inspection derives a typed
  incomplete-attempt result without rewriting any row.

Rows are protected by no-update and no-delete triggers. Their checksum uses a
domain-separated canonical JSON tuple. Every store operation verifies all fact,
receipt and audit checksums before returning data or committing another mutation.
*Amended 2026-09-17 (lots V2-R and V2-S):* every store operation verifies the
schema shape, the last verified row of each table, that no table has a row before
rowid 1, every row appended since, which must continue its table's rowids, the facts
it loads, and any stored receipt or audit row it replays or extends, before
returning data or committing another mutation; every fact, receipt and audit row,
and the rowids of each table, are verified at a process's first open of the store
and by a periodic complete verification. The triggers do not stop `REPLACE`, whose
implicit delete fires no trigger: an in-place change of an older row that no
operation reads, a replaced row or a removed row that leaves a gap included, is
detected at the next complete verification; a table whose last rows were removed
still verifies when no remaining row depends on them. Any failure to read or verify a stored row closes the
database file for the rest of the process, which commits no transaction on it
afterwards. See [MANAGER-PRE-REPLY-VERIFICATION.md](MANAGER-PRE-REPLY-VERIFICATION.md).
Accepted inbound-import audit rows must reference the receipt inserted in the same
transaction. The read-only verifier rejects a missing or mismatched reference.
One sequence validator applies before insert (to the stored rows of the
candidate's attempt), after load and while deriving typed
incomplete attempts. An authenticated accepted request observation has exactly one
durable decision: `inbound_import_committed` with its authenticated-import receipt,
or `inbound_refusal_recorded` with `authenticated_refusal` and a typed reason. The
decision precedes `inbound_reply_prepared`. A terminal
`inbound_reply_write_observed` must retain the same authenticated peer and receipt,
and its accepted or refused outcome and reason must match the durable decision.
An unauthenticated diagnostic, malformed request or unavailable request cannot
become an accepted or authenticated-refusal chain. An unsigned diagnostic reply,
whether complete or partially written, ends through
`inbound_diagnostic_reply_written`, with exact reply bytes, announced size and
digest. A connection that sends no reply ends through a matching
`inbound_connection_closed`, with zero reply bytes. A close after a prepared signed
reply also records unavailability, zero reply bytes and the same authenticated
peer. A diagnostic write cannot follow a durable decision or prepared signed reply.
A store failure that prevents recording a decision or close produces no durable
event and authorizes no signed reply.

A `preauth:` nonce identifies only one local inbound connection attempt before a
peer protocol nonce has been decoded. It is generated locally, remains unchanged
through that attempt's observation and diagnostic or no-reply terminal, and is
never replaced by invented peer data or by a later audit row in the same attempt.
It can carry no authenticated peer, peer claim, wire operation, local or remote
receipt, replay or effect authority. Once a decoded peer nonce exists, the
network layer uses that validated nonce for a separate attempt rather than
rewriting the pre-authentication attempt.

Byte counts are per-phase actual transferred bytes. Request observation carries
only request bytes; signed and diagnostic reply-write phases carry only reply
bytes. Import, refusal-decision and close phases carry zero bytes and no frame intent. Prepared
phases carry zero transferred bytes but retain the bounded body digest and
announced size of the request or reply they intend to transfer. A complete
transfer phase must retain that prepared digest and announced size, and its
actual byte count must equal the exact announced framed size. A partially written
diagnostic carries one through total-minus-one bytes and only the `unavailable`
outcome and error category. Its complete form retains the outcome, category and
reason of the observed diagnostic; after an authenticated accepted observation,
both partial and complete diagnostic failure evidence retain the same authenticated
peer and `unavailable` semantics. A failed signed reply write uses
`inbound_connection_closed` at zero bytes, or
`inbound_reply_write_observed` with `unavailable` for one through total-minus-one
bytes. A full signed frame is terminal only as its accepted or authenticated-refusal
outcome. Accepted outbound completion requires the
deterministic destination receipt identity derived from the logical manager,
source replica and wire operation.

The table is intentionally append-only and has no retention mechanism in this
increment. The G2 campaign therefore uses bounded fixtures and publishes exact
row counts. Production activation remains forbidden until quota, retention and
incident-preservation rules are designed.

## Receipt-bearing durable API

Add public typed evidence to the durable crate:

```rust
pub struct ReceiptEvidence {
    pub operation_id: String, // destination-local durable ID
    pub kind: ReceiptKind,
    pub source_replica_id: Option<String>,
    pub wire_operation_id: Option<String>,
    pub sha256: String,
}

pub struct Executed {
    pub response: Response,
    pub receipt: Option<ReceiptEvidence>,
    pub replayed: bool,
}
```

Add `Store::execute_with_receipt`. It preserves the existing idempotency contract
and returns the original receipt evidence on an identical replay. `Store::execute`
may remain as a compatibility wrapper returning only `response`.

Add one typed authenticated-import API that receives `wire_operation_id`, the
authenticated peer `Snapshot`, and a validated inbound audit event. It derives a
bounded local receipt ID as
`network:<sha256(domain, logical_manager_id, source_replica_id, wire_operation_id)>`.
The wire operation, local receipt and any remote receipt remain separate fields.
The `network:` namespace is reserved for authenticated imports; observations and
laboratory imports cannot claim it. Laboratory operation IDs use a bounded
validated compatibility grammar.
The API commits, in one SQLite IMMEDIATE transaction:

1. every newly accepted immutable fact;
2. the import mutation receipt; and
3. the `inbound_import_committed` audit event referencing that receipt.

Add a typed `Store::record_exchange_audit` operation for phases that do not share
a fact mutation transaction. It validates identities, enums, token bounds, byte
counts, digest shape and receipt references. An identical audit-event replay may
return the original row; reuse with different bytes must refuse. Only the atomic
authenticated-import API may normalize the stored `replayed` flag when the same
durable receipt is returned on a retry.

The durable boundary exposes `DurableError::{Refused, Corrupt, Storage,
InvalidAudit}`. `Refused` carries the closed `RefusalReason` enum. The network
stage must branch on this type; it must never infer a signed refusal from text or
turn `Storage` into a peer decision.

`transport_unavailable` is the stable reason for unavailable transport, reply
write and no-reply-close audit evidence. `unsafe_store` is reserved for an actual
unsafe canonical database or sidecar path and is not transport evidence. A
signed authenticated refusal decision remains limited to `invalid_request`,
`policy_violation` and `operation_id_reused`; neither local store safety nor
transport availability can be authenticated as a peer-request refusal.
Malformed and unauthenticated diagnostic outcomes accept only the malformed
category with `invalid_request`. They cannot carry a local store, identity,
policy, operation-reuse or transport reason, whether inserted live or recovered
from stored audit evidence.

No generic JSON-to-table or caller-supplied SQL interface is allowed.

## Canonical read-only store verification

Add a durable API similar to:

```rust
pub fn inspect_read_only(
    path: &Path,
    configuration: &Configuration,
    replica_id: &str,
) -> Result<CanonicalStoreInspection>;
```

It first captures stable database, WAL and SHM bytes into an atomically created
mode-0700 private directory, then uses SQLite read-only flags and a deferred
consistent transaction on that private copy. *Amended 2026-09-17 (lot V2-S):* the
capture reads the files twice and keeps them only when both reads are equal; a store
whose files total more than 16 MiB is copied into the private directory and read
again against that copy instead of twice into memory, so the capture's memory does
not grow with the store. If raw copied WAL state cannot be
opened, it captures another private snapshot and runs `VACUUM INTO` from that
private copy into a second private file; the fallback never opens the canonical
source through SQLite. After one successful preflight, a process cache bound to
device, inode, size, modification time, change time, topology and replica can
avoid repeating unchanged source preflight work within one process. In-place
replacement and different identities cannot reuse that entry. *Amended 2026-09-17
(lot V2-R):* a write operation that starts from a preflighted state also records the
state its own commit and checkpoint left; a change made by anyone else between two
operations of the process still requires a new preflight. The later canonical
read-write open uses SQLite `NOFOLLOW` and rechecks path metadata identity. It
never opens a refused old or mismatched canonical store read-write. It requires an
existing non-symlink regular database, schema v3 and the exact configured
topology/replica identity. Database and sidecar capture uses Linux
`O_NOFOLLOW|O_NONBLOCK`, checks regular-file identity before and after reading,
and fails closed on symlinks, FIFOs and other non-regular paths. It runs SQLite
integrity checking, verifies all stored checksums and receipt links, and
reconstructs the reducer through the existing typed model.

The canonical result includes:

- schema version, logical manager ID and replica ID;
- history count, ordered facts and a domain-separated logical-history digest;
- current facts, conflicts and blocked exclusive resources;
- receipt count, ordered receipt evidence and receipt-set digest;
- audit-event count, ordered bounded audit evidence and audit-set digest;
- ordered typed incomplete attempts containing direction, local attempt ID,
  wire nonce, wire operation and last durable phase;
- import receipts that lack a committed inbound-import audit; and
- the SQLite integrity result.

The logical-history digest is comparable across converged replicas. Receipt and
audit digests are local evidence and are not expected to match across hosts.

Stage D exposes the external process boundary as:

```text
podmesh-manager-ha-lab --inspect-store DATABASE CONFIGURATION_JSON_FILE REPLICA_ID
```

Stage R must map the same read-only result into the eventual installed form
`podmesh-managerd --inspect-store --config ... --state-dir ...`; that installed
mapping is not claimed by Stage D.

*Added 2026-09-17 (lot V2-S):* `podmesh-managerd --inspect-store --facts-only
--config ... --state-dir ...` (durable API `inspect_facts_read_only`) captures the
same private copy and verifies what the facts rest on: schema version and shape,
identity, the rowids of `facts` and every fact's checksum, JSON, event identity and
reducer validation. It prints `history_count`, `ordered_facts` and
`logical_history_sha256`, with the values of the full inspection, reads no receipt
or audit row and runs no SQLite integrity check, so neither its output nor its
verification grows with the audit table. It is the administration app's facts
reader, not an integrity verdict on the store.

Inspection does not require network opt-in or a runtime directory. It must not
create a missing database, lock, socket, WAL or SHM file. It exits nonzero on a
missing store, incompatible schema, identity mismatch, integrity error, checksum
error or receipt-link error, and *(lot V2-S)* on a table whose rowids are not
contiguous from 1. `--validate-config` remains separate and continues
to report `durable_store_checked: false`.

The qualification harness invokes this supported interface from a separate
process. It does not use direct SQLite queries as the acceptance proof.

## Authenticated network receipts and refusals

`experiments/manager-network/src/lib.rs` retains protocol ownership.

Extend a successful `WireReply::Imported` with:

- durable receipt operation ID;
- durable receipt checksum; and
- replay flag.

The reply HMAC binds those fields together with source/destination replica IDs,
operation ID, nonce, inserted count and history length.

Add a signed refusal reply for a request that has already passed configured-peer
lookup and request-MAC verification. It binds:

- source and destination replica IDs;
- operation ID and nonce;
- request-body digest; and
- a stable refusal reason code.

Pre-authentication failures continue to use an unsigned diagnostic. Add an error
source classification that distinguishes `AuthenticatedRemoteRefusal` from
`UnauthenticatedRemoteDiagnostic` and `Local`.

Refactor framed reads and writes to return actual byte counts on both success and
partial failure. Counts include the four-byte prefix. A declared oversized frame
records four bytes actually received and its announced length; it must never be
recorded as if the body arrived.

Outbound processing order:

1. export and encode a bounded request;
2. durably record `outbound_request_prepared` before connecting;
3. perform bounded connect, write and read;
4. verify an accepted reply or authenticated refusal;
5. durably record `outbound_exchange_completed`; and
6. only then report authenticated success to the resident status counters.

Inbound processing order:

1. read and meter one bounded frame;
2. decode, bind identities and authenticate when possible;
3. durably record `inbound_request_observed`;
4. for an authenticated request, atomically import plus receipt plus
   `inbound_import_committed`, or durably record `inbound_refusal_recorded` with
   the authenticated peer-request refusal reason;
5. encode and durably record `inbound_reply_prepared` before any signed reply;
6. write the reply; and
7. durably record `inbound_reply_write_observed` with actual signed reply bytes,
   or `inbound_diagnostic_reply_written` with the exact complete or partial
   unsigned diagnostic reply bytes when no durable decision or signed preparation
   exists.

If a required audit commit fails, the process must not report success. If an
inbound refusal cannot be durably recorded, close the connection without sending
a reply that claims a recorded decision. Signed refusal rows use only
`invalid_request`, `policy_violation` or `operation_id_reused`; local store and
configuration failures are not signable refusal reasons. When the audit store
remains writable, a complete or partially written unsigned diagnostic ends with
`inbound_diagnostic_reply_written`; a partial write is `unavailable` and retains
the intended frame digest and announced body size. A zero-byte close, including
one after reply preparation, ends with `inbound_connection_closed`.
When the store failure itself prevents that append, the connection still closes,
but no durable close evidence is claimed.

## Authorized resident observation append

Add `observation_writer_uid` to
`experiments/manager-resident/src/lib.rs::Configuration`. Validate the connecting
Unix peer credential with `rustix::net::sockopt::socket_peercred`. The packaged
three-host configuration uses UID `0`; process tests use their test-process UID.

Add exactly this mutation to the private control protocol:

```json
{
  "operation": "append_observation",
  "operation_id": "qualified-operation-token",
  "scope": "declared-local-scope",
  "subject": "bounded-subject",
  "value": "bounded-value"
}
```

The resident constructs the durable request itself with
`exclusive_resource: null` and `active_claim: false`. Those fields are absent from
the control schema; attempts to supply them are refused as unknown fields. The
durable topology independently verifies that the local replica owns the scope.

Increase the raw local request limit from 256 to 32768 bytes while retaining the
absolute 250 ms read deadline. The larger envelope is required so a value of up
to 4096 UTF-8 bytes fits after JSON escaping and protocol fields. The response
ceiling is 32768 bytes. Resident validation also bounds the logical manager,
replica, host and grant identities used in an append receipt to the same 128-byte
safe grammars; without those bounds the response ceiling would not be sound.
Operation ID and subject use the durable safe-token grammar, are at most 128
bytes and cannot use the reserved `network:` operation namespace. Scope is a
bounded hierarchical path of safe-token segments separated by `/`, with no
empty, current-directory or parent-directory segment. Value is nonempty UTF-8
of at most 4096 bytes. Every bound is checked before store access. The
successful response returns the typed fact, receipt evidence and replay flag.

Live `status` is a bounded diagnostic response: it reports the replica identity,
peer counters, worker bounds and `activation_authority: false`, but never embeds
facts, receipts or audit history. Peer counters reset on restart. Canonical
durable evidence is obtained separately through the read-only `--inspect-store`
interface, so retained history cannot make live status unbounded or couple it to
network I/O. `shutdown` retains its existing behavior.

The 250 ms control budget is absolute across request read, append-result wait and
response write. SQLite work runs in one bounded-admission worker. If it has not
finished before the response reserve, the resident returns an uncertain outcome,
keeps the worker for shutdown drain and relies on the durable operation ID for a
safe identical retry. It never reports an unobserved late commit as a refusal or
success.

## Failure semantics

| Failure | Required result |
| --- | --- |
| Unauthorized Unix UID | Refuse before store access; no fact or receipt. |
| Unowned scope | Durable request refuses; no fact or receipt. |
| Exclusive fields in local request | Typed JSON refusal; no store access. |
| Storage failure during local append | No partial fact/receipt and no success response; report uncertain unless the durable layer proves a policy refusal. |
| Append worker already occupied | Report busy immediately; admit no new worker and make no claim about a prior uncertain operation. |
| Local append exceeds the control response budget | Return uncertain, keep the resident responsive, drain the admitted worker on shutdown and resolve by identical operation-ID retry. |
| Client disconnect after local commit | Outcome is uncertain; identical retry returns the original receipt and does not append again. |
| Operation ID reused with different request | Refuse with zero mutation. |
| Authenticated inbound import succeeds | Facts, receipt and accepted audit commit atomically before reply preparation. |
| Authenticated inbound import refuses | Zero imported facts; `inbound_refusal_recorded` with a typed reason precedes signed refusal preparation; the final write must match it. |
| Wrong key or unknown peer | Zero import; identity remains an untrusted claim; a complete or partial unsigned reply uses `inbound_diagnostic_reply_written`, while a no-reply close uses `inbound_connection_closed`; neither can become accepted or authenticated. |
| Writable audit store and no reply sent | `inbound_connection_closed` records only matching diagnostic, malformed or unavailable semantics with zero transferred reply bytes. |
| Writable audit store and complete unsigned diagnostic sent | `inbound_diagnostic_reply_written` records exact reply bytes, announced size and digest, without a receipt or durable accepted/refusal decision. |
| Writable audit store and partial unsigned diagnostic sent | `inbound_diagnostic_reply_written` records one through total-minus-one actual bytes, the intended digest and announced size, and only `unavailable` outcome/category; it remains terminal and carries no receipt or decision authority. |
| Audit storage fails before decision/close append | Close without signed reply and without falsely claiming durable decision or close evidence. |
| Reply is lost after destination commit | Destination retains import receipt and audit; source retains a prepared/incomplete attempt. Identical operation retry with a fresh nonce receives a replayed signed receipt. |
| Source receives signed success but cannot persist terminal audit | Source reports uncertain/local storage failure, not success. |
| Process dies after a prepared phase | Read-only inspection reports the attempt incomplete; recovery never invents a terminal outcome. |
| Corrupt fact, receipt or audit row | Mutation, replay and successful inspection all fail closed. *Amended 2026-09-17 (lots V2-R and V2-S):* replay of that row and successful inspection fail closed; mutation fails closed once the row is added since the last verification without removing a stored row, read by the operation, or found by a first open or a periodic complete verification, and the database file then stays closed for the process, which commits no transaction on it. |
| Missing database during inspection | Refuse without creating any file. |

## Test inventory

### Durable crate

- first mutation and identical retry return one exact receipt;
- different request under the same operation ID refuses;
- inbound facts, receipt and accepted audit commit atomically;
- an audit-ID collision after fact/receipt preparation rolls all three back;
- audit rows are immutable and checksum-verified;
- forged/corrupt audit rows and invalid receipt references fail closed;
- complete accepted and authenticated-refusal inbound chains preserve the exact
  durable decision through reply write;
- unauthenticated promotion refuses during insert and is corruption when found
  in stored rows;
- refusal recorded before a crash remains a typed incomplete attempt;
- diagnostic and unavailable no-reply closes cannot claim an authenticated
  result;
- complete and partial unsigned diagnostic replies have distinct exact-byte
  terminal evidence and cannot follow a durable or prepared signed decision;
- a partial diagnostic accepts only `unavailable` outcome/category, a full
  diagnostic retains its observation-compatible outcome, and stored violations
  fail as corruption;
- diagnostic peer binding is checked at a four-byte partial write and a complete
  frame; independent success tests cover one-byte, three-byte, four-byte and
  full-minus-one transfer boundaries;
- diagnostic byte validation rejects zero-byte writes, missing reply announced
  size, missing reply digest, and request evidence repeated as frame bytes,
  announced size alone or digest alone;
- validated local `preauth:<64 lowercase hexadecimal SHA-256>` nonces support authority-free malformed or
  unavailable observation followed by an unsigned diagnostic or zero-byte close;
  malformed nonces, authority fields, non-inbound phases and nonce replacement
  within one attempt refuse exactly;
- unavailable transport, reply-write and close evidence uses
  `transport_unavailable`; `unsafe_store` remains a local path-safety refusal and
  neither reason can become an authenticated peer-request refusal;
- malformed and unauthenticated diagnostic outcomes accept only
  the malformed category with `invalid_request`, with exact insertion rejection
  for every other closed reason and for unavailable/refused categories, plus
  validly checksummed stored-corruption regressions for reason and category;
- zero-byte signed reply failure uses a close, partial writes use unavailable
  reply-write evidence, and full frames use only accepted/refused outcomes;
- all meaningful inbound chain rejection branches have exact insert tests, with
  stored-corruption regressions for diagnostic and authenticated-close rules;
- malformed local import audit data is rejected before snapshot import logic;
- schema v2 refuses without rewrite;
- canonical private-copy read-only inspection works against a live WAL database;
- inspection leaves the database directory, database, WAL and SHM metadata and
  contents unchanged;
- missing, mismatched, symlinked and corrupt stores remain unmodified;
- a killed-child live WAL-mode v2 store and a mismatched live v3 WAL store are
  refused with database, WAL and SHM bytes preserved;
- receipt kind/source/wire metadata and unaudited laboratory imports are visible;
- local attempt identities remain distinct under a reused peer nonce;
- exact `sqlite_` internal-name exclusion, unexpected SQLite schema objects and
  zero-row inserts cannot produce success;
- stored fact, receipt and audit corruption has the exact `Corrupt` class while
  unreadable SQLite content has the `Storage` class;
- local non-import operations cannot squat the authenticated-import receipt
  namespace;
- authenticated terminal phases reject unauthenticated predecessors;
- prepared, partial outbound and complete inbound byte semantics are enforced;
- audit replay normalization is restricted to atomic authenticated import;
- symlink and FIFO sidecars fail closed without blocking; and
- *(lot V2-S)* a row added before a table's first row or after a gap is refused by
  the next transaction; a row replaced in place is found by the complete
  verification; an unreadable row read at use, and a schema found corrupt at open,
  close the store; no transaction commits once the store is closed.

### Network crate

- accepted reply binds receipt ID, checksum, replay flag and request identity;
- authenticated post-MAC refusal verifies as an authenticated refusal;
- wrong-key response remains an unauthenticated diagnostic;
- unknown peer, malformed JSON, truncation and oversized frames record actual
  byte counts and import nothing;
- an authenticated snapshot with a later invalid fact commits no partial import;
- reply loss leaves the destination receipt/audit and an incomplete source
  attempt;
- identical retry obtains the original remote receipt with no duplicate fact;
- changed content under the same operation ID refuses and is audited; and
- audit-store failure cannot produce a success result.

### Resident crate

- configured UID appends one non-exclusive observation through the Unix API;
- another UID, another replica's scope and unknown/exclusive fields refuse;
- a repeated local operation returns the same fact and receipt;
- three resident processes append one owned observation each through the Unix API
  and converge;
- canonical read-only inspection reports the same logical-history digest on all
  three stores;
- clean restart preserves replica identity, receipts, audit events and next
  producer sequence;
- partitioned replicas retain local owned observations and converge after
  reconnection;
- accepted/refused exchanges contain the required audit phases and byte counts;
- worker/admission bounds and existing framing deadlines still pass;
- `activation_authority` remains false in every status result; and
- *(lot V2-S)* `--inspect-store --facts-only` prints the full inspection's facts,
  reads no audit row, refuses a corrupt fact and is refused without
  `--inspect-store`.

Run locked tests, clippy with warnings denied and formatting checks for all three
crates. Re-run packaging tests and the complete manager qualification harness.

## Package and three-host qualification

Build a new experimental `podmesh-manager` package revision. Do not reuse the
installed `manager1` version because the binary, configuration and schema contract
change.

Update the package example with `observation_writer_uid: 0`. No maintainer script
may generate identities, keys or configuration, start the service, add network
rules or migrate a store. The service remains disabled and network-disabled after
package upgrade.

Qualification order:

1. compile and test the exact candidate;
2. run the package and qualification harnesses;
3. obtain an independent Claude Code counter-review, verify every finding with
   Codex, correct and rerun affected tests;
4. back up the repository container and preserve the previous signed package;
5. publish the new candidate through signed experimental APT;
6. upgrade all three hosts without automatic restart;
7. install protected configurations with one logical manager ID, three distinct
   host/replica IDs, distinct pair keys, one owned non-overlapping scope per
   replica and writer UID zero;
8. repeat offline configuration validation and default-disabled failed-start
   proof;
9. install the separately reviewed systemd network drop-in with only `AF_UNIX`
   and `AF_INET`, inherited deny-all policy, and exact peer `/32` allowances;
10. activate only `podmesh-manager.service` on each host;
11. append one non-exclusive fact per host through the Unix API;
12. preserve external request/reply output and canonical read-only store output;
13. execute HA-01, HA-02 and the G2-only portions of HA-05 through HA-08;
14. stop all three manager services and inspect the closed stores again; and
15. prove lifecycle/observer services, package versions and rootful Podman
    commitments outside the manager remain unchanged.

Each evidence bundle records source commit, package and binary hashes,
configuration hash without secret content, host/replica/logical-manager IDs,
service process evidence, operation and attempt IDs, framed byte counts, receipt
and audit checksums, canonical history digest, SQLite integrity, injected fault,
cleanup and retained artifact hashes.

G2 passes only when all three stores contain the expected immutable facts, have
the same canonical logical-history digest, retain coherent receipt/audit evidence,
and pass independent read-only inspection after restart and reconnection. This
result is reported as **qualified laboratory authenticated durable exchange**,
never as manager HA.

## Staged file ownership

Only one constructing owner edits a stage at a time. Later stages consume the
committed result of earlier stages. Counter-review remains read-only until findings
are assigned back to the owning stage.

| Stage | Owned files | Deliverable |
| --- | --- | --- |
| D — durable core | `experiments/manager-ha/src/durable.rs`, `experiments/manager-ha/tests/process.rs`, manager-ha README/evidence | Schema v3, receipt-bearing execution, typed audit API and canonical read-only inspection. |
| N — network protocol | `experiments/manager-network/src/lib.rs`, `experiments/manager-network/src/main.rs`, network tests, README/evidence | Metered framing, receipt-bound success, authenticated refusal and durable exchange phases. |
| R — resident interface | `experiments/manager-resident/src/lib.rs`, `src/cli.rs`, resident tests, README/evidence | UID-bound non-exclusive append, read-only CLI and durable-status integration. |
| P — package | `packaging/podmesh-manager/**`, package-specific deployment documentation | Reproducible new candidate with updated example and unchanged safe installation policy. |
| Q — qualification | Manager qualification harness and private evidence scripts only | Three-host execution, external observations, comparisons and cleanup proof. |
| C — closeout | Acceptance/checklist/review documents | Reconciled scope inventory, three-pass closeout and exact qualified/open claims. |

Stage D freezes the schema and durable types before Stage N changes protocol
serialization. Stage N freezes the wire reply/refusal contract before Stage R
connects it to the long-running service. Stage P packages only a reviewed,
fully tested Stage R binary. Stage Q never edits product code while collecting
evidence. A code correction returns to its owning stage, receives a regression
test in the same change, and restarts every affected downstream stage.

## Completion boundary

This document is complete when it enables implementation and review without an
architectural guess. G2 itself remains open until the exact installed package is
exercised on all three laboratory hosts and the evidence contract passes.

Even after G2 passes, G3 effect exclusion, G4 remaining lifecycle requirements,
G5 manager recovery, G6 DNS/bootstrap recovery, G7 Governor/Maker operation,
transport confidentiality, key lifecycle, retention and ShaperOS integration all
remain separate open work.
