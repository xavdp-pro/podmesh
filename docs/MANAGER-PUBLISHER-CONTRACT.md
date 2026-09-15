# The publishing connector follows the governor

Status: **contract written 2026-09-15 on the operator's decision**
(`CLOUDFLARE-TUNNEL-MANAGER-HA-DECISION-2026-09-15`); what is built is stated in "What is built"
at the end, and nothing above it is a claim about code.

The logical manager's human-facing interface is published through a Cloudflare Tunnel. That must
not create a second election or a second authority: the connector is transport, it decides
nothing, and a connector being online grants no manager authority.

## Invariant

One public hostname, one logical tunnel (one tunnel UUID), three manager replicas, and — in this
first candidate — **exactly one publishing `cloudflared`**, co-located with the governor replica
and governed by the same resource (the logical manager UUID), the same epoch and the same fence
as the service address (`UNIVERSE-NETWORK-CONTRACT.md`, `UNIVERSE-HIGH-AVAILABILITY.md`).

Cloudflare accepts several connectors for one tunnel and does not say which receives a request;
three connectors each pointing at their local replica would let a standby receive a write.
Several active connectors are allowed later only when every one of them proxies to the same
stable, epoch-qualified service address and fails closed when that origin is not current.

## Identities and what stays where

| what | where | never |
| --- | --- | --- |
| tunnel UUID, hostname, credential's name, origin port | the host's journal (`publisher_declare`) | — |
| the tunnel credential | Podman's secret store on the host (`secret_declare`), a root-only runtime copy while the connector runs | Git, an image layer, the journal, the replicated store, a recovery point |
| the connector's identity | `cloudflared`'s own journal (`connection=<id>`) | the journal as truth |
| the governor mark (resource, epoch, marked at) | a root-only file inside the carrier universe, written and removed by PodMesh | anywhere the universe could write it itself |

## The origin, epoch-qualified

The manager universe answers `GET /ready` on its origin port (8080) with its logical manager
and replica identities and, **only while the governor mark is present**, the epoch it was marked
with — HTTP 200; without the mark, or on any other path, HTTP 503. The mark is PodMesh's: written
at `publisher_start` under the epoch gate, removed at `publisher_stop` and by the fence. A
connector that reaches a replica that is not the governor gets nothing. The manager's web
interface is not served there yet; this responder is the origin the contract requires today.

## Operations

All journaled, all recorded in the network effects ledger before they are made and reconciled
with it (`UNIVERSE-NETWORK-CONTRACT.md`, "Failure and cleanup states"), all with
`authorization_ref` as provenance.

- `publisher_declare` (`resource`, `hostname`, `tunnel_uuid`, `credential`, optional
  `origin_port`): recorded by reference; refused without an activation policy for the resource
  on this host, or without the credential declared.
- `publisher_start` (`resource`, `previous`): refused, in this order, when the resource's lease
  is not live and unsuperseded here (the lease gate's four reasons), when the exclusive route and
  the alias are not effective on this host, when the credential is not in Podman's store, and
  when the previous publisher is not accounted for — `previous` names `{fenced: true, …}`,
  `{waited_seconds: n}` (the lease plus margin waited for an unreachable host) or `{none: true}`
  (the first publisher): the agent's word, recorded, refused when absent. Then, each step
  recorded first and verified: the governor mark written inside the carrier universe; the origin
  asked at the service address and required to answer ready with the expected logical manager,
  the carrier's replica (as its resident names it through the control door) and the lease's
  epoch; the connector started as a transient unit from a root-only runtime copy of the
  credential and an ingress from the hostname to the service address, and verified active. A
  failure undoes what was made, last first, and reports the compensation.
- `publisher_stop` (`resource`): the connector stopped and the mark removed, verified.
- `activation_fence`: for every resource this host no longer holds, the publisher is withdrawn
  first — connector stopped, mark removed — **before** the alias and the route go: one
  transition, each step recorded and verified, reported as `publishers_withdrawn`.
  `activation_fence_preview` counts a connector without entitlement as pending.
- `publisher_observed` (`resource`, `observation`): the agent records an external request's
  result — what the public hostname answered — as provenance.
- `publisher_status` (read-only; `resource`): what is declared; the unit's state; the
  connector's identity from its journal; the lease and epoch; the service address and its
  carrier; the carrier's replica identity through the control door; the origin's readiness now;
  the governor mark; `publisher_eligible` with the refusal reasons; the last start, stop, fence
  and observed request.

## Takeover, in order

1. Rotate the epoch through the external gate.
2. Deliver the supersession and fence the previous governor (or, unreachable, wait its lease plus
   margin: the timer on it withdraws its connector, its mark, its alias and its route on its own
   clock — `packaging/podmesh-fence`).
3. Observe on the previous governor, when reachable, the connector stopped and the address gone.
4. Publish the service address on the new governor (`network_route_publish`, exclusive).
5. `publisher_start` there: readiness at the new epoch, then the connector.
6. Verify the connector's identity and exercise the public hostname from outside; record it
   (`publisher_observed`).

No DNS mutation is part of a takeover: the hostname is a CNAME to the tunnel, and the tunnel is
the same. A connector whose local origin is absent, stale, wrong-epoch or unauthenticated stops
or refuses rather than proxying elsewhere.

## What this contract does not decide

The manager's web interface behind the origin; Cloudflare's own availability; several active
connectors (later, with their own qualification); out-of-band fencing of a host whose daemon is
wedged (lease-expiry self-withdrawal needs the daemon alive).

## What is built

See the entries below, each dated, each measured on the laboratory's hosts.
