# Replicated logical manager laboratory model

Status: isolated deterministic model. It is not a deployed manager, a network
protocol, a production failover service or proof of high availability.

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
| MH-08 | Incomplete or forked history quarantine | Missing predecessor and same-revision fork block the affected subject | Tested locally |

Run:

```sh
cargo test --locked --manifest-path experiments/manager-ha/Cargo.toml
cargo clippy --locked --all-targets --manifest-path experiments/manager-ha/Cargo.toml -- -D warnings
cargo fmt --manifest-path experiments/manager-ha/Cargo.toml -- --check
```

## What this proves

The tests prove deterministic behavior of an in-memory three-replica model:
partitioned writes stay within statically declared non-overlapping scopes,
reconnection requires identical exchanged histories, coordinator selection does
not determine truth, stale copies catch up, exclusive conflicts block their
resource, and inactive copies cannot advertise a modeled service.

This is useful as an executable contract for the later registry reducer and
manager service. It models the intended move away from one shared live control
filesystem without pretending that this local experiment proves availability or
that replicated facts remove the need for authority over exclusive effects.

## What remains unproven

This crate has no SQLite persistence, authenticated origin, authenticated network
transport, WireGuard integration, discovery, DNS server, Podman control, real
service IP, leases, clocks, failure detector, fencing, crash recovery, power-loss
test, host deployment or external observer. Its scope grants are static fixtures.
Its permit is an in-process value, not a durable authorization or a fence against
an old process.

The current coordinator tie-break compares stable replica IDs bytewise. A later
protocol must freeze the ID format or introduce a separate immutable priority;
this experiment does not interpret human-looking numeric suffixes.

Therefore it makes no claim of quorum-free automatic takeover, split-brain
prevention on real hosts, instant failover, zero data loss, production HA or
availability during full control-plane loss. A real takeover must add authenticated
membership, durable history exchange, failure evidence, exclusive authority or
fencing, and proof that an old active instance cannot continue to announce or act.

The next integration step is to map these rules onto the durable observation store
without treating imported enrollment as authority, then qualify three actual
control-service replicas under partition, reconnect, stale return and host loss.
