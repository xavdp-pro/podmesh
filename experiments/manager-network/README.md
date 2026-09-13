# Durable authenticated manager exchange laboratory

Status: Stage N is coded and locally tested against the Stage D durable API. It
is an isolated plaintext-TCP laboratory, not Stage R, a complete G2 delivery, a
package, an installed-host result, or manager high availability.

## Boundary

One locally configured replica exports its typed immutable snapshot and sends it
to one exact configured peer. Network input cannot select a database, topology,
peer, key, route, service, or effect. The transport does not enroll peers,
publish DNS, activate an address, run Podman, perform takeover, or fence another
replica. `activation_authority` remains outside this crate and false in the
resident laboratory.

Each configuration contains the local replica identity and SQLite path, the
complete durable manager topology, one bind address, and exactly one entry for
every other replica. Pair keys are distinct 32-byte lowercase hexadecimal values.
The same application HMAC is required whether the endpoint is direct or routed
through WireGuard. Plain TCP provides no payload confidentiality.

## Frame contract

Each JSON body has one four-byte big-endian length prefix. The maximum body is
512 KiB. Reads and writes use one absolute two-second deadline across prefix and
body and retry `Interrupted` without renewing that deadline.

Transfer evidence records actual bytes, including the prefix, and records the
announced body length once all four prefix bytes arrive. A body SHA-256 exists
only after a complete read. A write records the digest of its complete intended
body even when the peer accepts only a prefix. An oversized announcement stops
after four bytes and never allocates the announced body. There is no transport
flush after a complete frame, so a completed write cannot later be reclassified
as unavailable by an ambiguous flush result.

Tests cover exact 0, 1, 3, 4, total-minus-one, and total byte boundaries for reads
and writes, oversized prefixes, interrupted calls, and a trickled absolute read
deadline.

## Identity and authentication

The HMAC domain ends in an actual NUL byte, separates this protocol, and binds
every request field:

- protocol, source replica, destination replica, wire operation, nonce; and
- the complete typed snapshot.

An accepted reply binds source, destination, wire operation, nonce, the exact
sent request-body SHA-256, inserted and history counts, destination receipt
operation ID, receipt checksum, replay flag, and its MAC. The source also verifies
the deterministic Stage D receipt mapping, checksum shape, count bounds, and
exact request digest.

After request-MAC verification, the destination may sign only
`invalid_request`, `policy_violation`, or `operation_id_reused`. A signed refusal
also binds the complete received request-body digest. A wrong key, unknown peer,
malformed message, or other pre-authentication failure receives at most an
unsigned diagnostic. The public error source distinguishes `Local`,
`AuthenticatedRemoteRefusal`, and `UnauthenticatedRemoteDiagnostic`.

The caller keeps one stable wire operation for an identical logical retry and
uses a fresh nonce for every transport attempt. `sync_to` validates nonce shape
but does not persist a nonce-reuse registry. Security does not depend on nonce
uniqueness: both signed reply variants bind the exact request digest, so a
captured success cannot authenticate a changed request even if operation and
nonce are reused. Stage D creates a fresh local
`attempt:<sha256>` for each attempt. The destination derives its own bounded
`network:<sha256>` receipt through `execute_authenticated_import`; wire operation,
attempt, nonce, and receipt identities remain separate. Reusing a wire operation
with changed snapshot content returns an authenticated `operation_id_reused`
refusal with no partial import.

## Durable sequence

Outbound order is fixed:

1. export, sign, and encode one bounded request;
2. append `outbound_request_prepared` before connect or write;
3. meter connect, request write, and reply read;
4. verify every signed reply field; and
5. append exactly one `outbound_exchange_completed` before returning, except
   when a complete request has total reply loss.

Connect, partial write, partial or malformed reply, bad MAC, binding failure,
unsigned diagnostic, authenticated refusal, and accepted reply receive terminal
classification with actual phase byte counts. Any unavailable reply read with
zero transferred bytes, including EOF, timeout, or reset, is uncertainty rather
than malformed input; Stage D deliberately retains the source
`outbound_request_prepared` attempt without a terminal event. If a signed success
arrives but the source cannot append its terminal audit, `sync_to` returns local
uncertainty.

Inbound order is fixed:

1. allocate a local pre-authentication nonce and meter one frame;
2. decode a validated wire nonce into a fresh local attempt when possible;
3. append `inbound_request_observed`;
4. atomically import facts, receipt, and `inbound_import_committed`, or append one
   allowed `inbound_refusal_recorded` decision;
5. append `inbound_reply_prepared` before a signed reply; and
6. append exact full, partial, or zero-write terminal evidence.

A pre-authentication attempt never changes nonce and carries no authenticated
peer, operation, receipt, or replay authority. Unsigned diagnostics cannot follow
a durable accepted/refused decision. A non-signable durable error before a
decision records a zero-byte authenticated close when the store remains writable.
Stage D requires that terminal to use `transport_unavailable`; the returned local
error retains the actual durable cause. An unwritable store leaves the attempt
incomplete and the original error is never masked. Missing receipts and signing,
encoding, or preparation failures after a durable decision remain incomplete
because Stage D does not permit a close before a matching signed-reply
preparation. No signed reply is sent in any of these cases. The deliberate
process-test reply-loss seam stops after the atomic
decision, leaving visible incomplete destination evidence for retry qualification.

`serve-once` checks one absolute admission deadline before every accept and stops
after at most eight connections. A connection queued before the cutoff is not
accepted after it. This deadline bounds admission only: each connection accepted
before the cutoff receives its own bounded frame read/write deadline and may
finish after the admission cutoff. An unauthenticated connection receives at most
an unsigned diagnostic. Fast-failing bad connections do not consume the service,
but one stalled connection can exhaust its I/O budget and the admission deadline;
the listener then intentionally rejects any queued connection. The cap also ends
admission after eight connections.

`DurableError` is matched by variant. `Refused` retains its typed reason;
`Corrupt`, `Storage`, and `InvalidAudit` remain distinct local classes. Store
safety, schema, identity, corruption, audit, and transport failures never become
signed peer-request refusals.

## Commands

```sh
cargo test --locked --manifest-path experiments/manager-network/Cargo.toml
cargo clippy --locked --manifest-path experiments/manager-network/Cargo.toml --all-targets --all-features -- -D warnings
cargo fmt --all --manifest-path experiments/manager-network/Cargo.toml -- --check
```

The executable accepts:

```text
podmesh-manager-network-lab serve-once CONFIG
podmesh-manager-network-lab sync CONFIG PEER OPERATION_ID NONCE
```

`serve-once-ready` binds the configured address, prints the still-owned actual
address, then begins bounded admission. Tests use it with port zero so there is
no bind-drop-rebind reservation race. `serve-once-drop-reply-ready` is a
laboratory-only fault seam for reply loss after a durable decision. It refuses to
run unless `PODMESH_MANAGER_NETWORK_LAB_ENABLE_REPLY_LOSS=1` is explicitly set.
The public library method remains an intentional laboratory-only fault API because
the process proof must invoke it from a normal binary build; it is not exposed by
an installed package or service.

The listener itself is nonblocking. The qualified Linux runtime does not pass
that mode to an accepted socket, so per-connection blocking reads use their
configured deadlines. Portability to systems where accepted sockets inherit
nonblocking mode, including BSD-family behavior, is not qualified by this
evidence.

## Evidence and limits

The process proof uses real child processes, retained state, reply loss, exact
replay with a fresh nonce, catch-up to a third replica, and the external
`podmesh-manager-ha-lab --inspect-store` interface. It verifies SQLite integrity,
no unaudited authenticated imports, expected terminal and incomplete attempts,
and one common logical-history digest.

Stage N retains full snapshots and pairwise HMAC keys in protected local
configuration. It has no encryption, production key rotation/revocation, dynamic
membership, incremental cursor, paging, quota, rate limiting, long-running
listener, installed service, three-host deployment, takeover, fencing,
split-brain prevention, DNS, or effect authority. See `EVIDENCE.md` for the exact
local qualification results.
