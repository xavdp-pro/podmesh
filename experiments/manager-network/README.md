# Authenticated manager replica transport laboratory

Status: isolated local-TCP transport experiment. It maps an authenticated replica
snapshot into the existing `experiments/manager-ha` durable `Import` request.
It is not deployed, packaged, enrolled dynamically, a manager failover service,
or evidence of high availability.

## Purpose

The control-services universe is one logical manager with one replica per host.
The durable manager experiment already binds immutable facts to a configured
topology and imports them atomically. This increment adds a real process and TCP
boundary without changing that experiment:

1. the local replica exports its typed durable snapshot;
2. it signs a bounded exchange request for one preconfigured peer;
3. the receiving process verifies its configured peer identity and shared key;
4. it maps the request to `durable::Request::Import` with a namespaced durable
   operation ID; and
5. it sends a peer-authenticated receipt only after the durable import commits.

The connection can use a direct LAN endpoint or an endpoint carried through
WireGuard. WireGuard is optional: it can protect routing and transport paths, but
the application-level request/reply MAC remains required. Incoming network data
never names a SQLite path, changes topology, adds a peer, or creates enrollment.

## Static configuration

Each local JSON configuration includes its replica ID, local SQLite path,
durable-manager topology, local bind address, and one exact peer record for every
other configured manager replica. A peer record has the peer replica ID, endpoint
and a distinct 32-byte hexadecimal pair key. A production key lifecycle is intentionally
outside this laboratory; test keys must never be reused.

The peer IDs must be complete, distinct, in the durable topology and different
from the local replica. The snapshot's replica ID and topology must both match
the authenticated peer and the recipient's pre-existing local configuration.
There is no discovery, enrollment, configuration replication, implicit bootstrap
or trust-on-first-use.

## Wire protocol and replay boundary

`podmesh-manager-network-lab/1` is a length-framed JSON request. The HMAC-SHA-256
input is domain-separated and binds the protocol, source and destination replica
IDs, operation ID, nonce, and complete typed snapshot. The successful reply binds
the same request identity plus the exact durable import result. This proves only
that a configured peer with the pair key accepted the request and committed its
local durable result.

The caller supplies an opaque operation ID and nonce. Both are one to 128 ASCII
letters, digits, hyphens or underscores. The recipient transforms an accepted
operation ID to `network/<source-replica-id>/<operation-id>` before calling the
durable store. The durable store's immutable operation receipt gives replay safety
across a clean process restart: an identical retry returns the original import
result; reuse with a different snapshot fails. The durable replay key deliberately
binds the authenticated source replica and operation ID plus the mapped snapshot;
the nonce is authenticated on the wire and bound to a successful reply, but is not
part of the durable receipt. A repeated operation ID with a new nonce therefore
replays only when its snapshot is identical. Callers must allocate fresh IDs
and cryptographically unpredictable nonces in any non-laboratory deployment.

Error replies are deliberately unauthenticated diagnostic status only; no caller
may treat them as peer authority or a management decision. The public error type
marks them as `UnauthenticatedRemoteDiagnostic`; locally derived errors are marked
`Local`. A successful import reply is mutually authenticated. An on-path actor
can still prevent delivery; availability and peer liveness are not proven.

## Bounds and outcome classes

| Boundary | Fixed value |
| --- | --- |
| One-shot listener connections | 1 per `serve-once` process |
| Request or reply JSON frame | 512 KiB maximum, excluding four-byte length |
| Listener accept, connect, read and write timeout | 2 seconds each |
| Initial connection retry window | 2 seconds, 100 ms attempts and 20 ms pause |
| Operation ID and nonce | 1–128 ASCII token characters |
| Local configuration JSON | 1 MiB maximum, read before parsing or opening SQLite |
| Imported snapshot | 512 KiB effective limit from this network frame |
| Durable manager core | No independent snapshot-byte limit; this experiment always applies its 512 KiB outer frame |
| Imported history | Existing manager-ha semantics; full snapshot only |

`unavailable` means a local listener or bounded I/O path could not be reached.
`refused` means a configured identity, MAC, topology, replay binding or durable
import was rejected. `malformed` means a frame, token or JSON request cannot be
interpreted within bounds. A malformed frame is refused before the durable import.
An import failure leaves no partial durable import because the mapped durable
request is one SQLite transaction.

## Run

```sh
cargo test --locked --manifest-path experiments/manager-network/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-network/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-network/Cargo.toml -- --check
```

The one-shot executable is intentionally explicit:

```sh
podmesh-manager-network-lab serve-once LOCAL-CONFIG.json
podmesh-manager-network-lab sync LOCAL-CONFIG.json PEER_ID OPERATION_ID NONCE
```

It does not start a resident service and does not run shell commands.

`Node::serve_connection(TcpStream)` handles exactly one accepted connection with
the same authentication, framing and atomic import. Its caller owns listener
admission/concurrency; `../manager-resident/` supplies that laboratory owner.
Frame reads/writes use absolute two-second deadlines, so trickled bytes cannot
renew a timeout forever.

## Evidence covered by tests

- a three-replica executable test holds two destination listeners while the
  third replica process catches both copies up from a prior offline state;
- exact configured identity and HMAC success, then identical replay receipt;
- unavailable peer reported separately from an unsigned remote refusal diagnostic;
- wrong-key peer refusal, truncated/frame-length refusal and oversize frame refusal
  with no durable mutation;
- a correctly authenticated snapshot containing a later invalid fact is refused
  with zero facts committed on the recipient; and
- reconnect/catch-up of a stale replica from a full snapshot.

## Explicit gaps

This experiment has no certificates or per-host key storage/rotation/revocation,
no encrypted application payload, no signed original facts, no peer discovery,
no TLS, no endpoint allowlist by network address, no incremental pagination,
no rate limiting, no long-running listener, no listener concurrency policy, no
clock-based replay window, no recovery for lost local durable receipts, no backup
or installation path, and no host deployment.

It does not perform a takeover, coordinator action, fencing, DNS/IP publication,
Podman operation, migration, service advertisement or failure detection. A
reachable authenticated replica is not evidence that another replica stopped.
This is a transport prerequisite for later HA qualification, not HA itself.
