# Control-services universe: purpose, replication and operating contract

Date: 2026-09-12. Audience: the PodMesh implementation and review agents.
Status: operator design direction. This is a design contract for later lots, not
an implementation claim, a SHAPER canon amendment, or proof of high availability.

## The problem it solves

PodMesh is being built because the operator wants autonomous Linux hosts that can
join, leave, be rebuilt, and reconnect without forcing every existing workload to
be evacuated first. The goal is more operational freedom than a cluster that
depends on one shared filesystem, one permanently available control node, or a
global quorum that can leave healthy hosts unable to work after a split.

Traditional shared-cluster designs commonly make control information and workload
storage depend on a shared distributed filesystem and quorum membership. They are
valuable designs, but a damaged quorum, a lost storage dependency, or an awkward
cluster join can turn recovery into a cluster-wide problem. PodMesh deliberately
does not begin by reproducing that dependency chain.

The replacement is not “no consistency”. It is a smaller and clearer division:

1. Every host remains able to operate its own local Podman runtime through its
   local Maker and PodMesh API.
2. A replicated **control-services universe** provides the shared logical services
   needed to discover, name, observe and coordinate universes.
3. Durable workload data, image availability, checkpoints and control records have
   separate replication and recovery contracts. A copied runtime process or a
   replicated database is not silently treated as a shared filesystem.
4. During a partition, each host acts only inside rights already delegated to it.
   Reconnection exchanges facts before coordination changes. No host infers that a
   remote universe has stopped merely because it cannot reach it.

## Name and role boundary

Older discussions call this component the “manager”. Do not use **manager** as an
unqualified governance role. SHAPER already has precise names:

| Term | Function | What it must not become |
| --- | --- | --- |
| Human / Steward | defines purpose, authority and major decisions | an operational bottleneck for every safe routine action |
| Governor | holds declared desired state, routes, coordinates and escalates | a host-root executor or a hidden source of authority |
| Maker | runs on or for one host, retrieves authorized work, invokes local operations and reports facts | an autonomous policy owner |
| PodMesh | local runtime mechanics: Podman inventory, lifecycle, migration, evidence | another governor or global scheduler |
| Control-services universe | a deployable package for registry, DNS, discovery, observation and related control services | the Governor, a Maker, or an authority grant by itself |

Use **control-services universe** in new architecture and implementation text.
For transition, “manager universe (control-services universe)” is acceptable when
linking to older documents. The SaaS remains the Governor. A replicated
control-services universe does not itself decide what should exist or obtain the
right to execute host actions.

## What runs in the control-services universe

The exact components remain selectable, but the logical function is stable:

- a registry of stable universe UUIDs, logical parents, assigned names, network
  identities, instance/host records, observed status, evidence references and
  delegated scopes;
- DNS or DNS publication based on those registry facts, so applications and parents
  use names rather than fixed host IP addresses;
- discovery/bootstrap information needed to find another available replica;
- observation and synchronization services that exchange dated facts and histories;
- optional operator-facing status APIs later, using the same authority contracts.

It does **not** need to be a remote root gateway to every host. It must not hold
unrestricted host credentials merely because it contains the registry or DNS. A
Maker keeps local execution capability; PodMesh keeps a local root-only API until
remote authorization is explicitly designed and qualified.

## Replication model

There is one logical control-services universe and one replica on each participating
host. “One logical universe” means the service identity, schema, records and
purpose are shared. It does not mean independent processes can write contradictory
facts without a protocol.

Each replica needs:

- a stable **logical service UUID**, shared by all replicas;
- a distinct **instance UUID** and current host UUID, so observations identify the
  exact process that made them;
- durable, versioned records with operation IDs, author, observed time and evidence
  references;
- a replication cursor/history rather than copying mutable database files;
- a bootstrap path that can locate a surviving replica without requiring its own DNS
  service to be available first.

Replicate records and explicitly defined recovery points. Do not mount the same
live database directory or filesystem on several hosts and call that replication.
That would reintroduce the shared-storage and recovery coupling this design is
trying to avoid.

## Operation while connected

When replicas can communicate, they exchange versioned registry facts and select a
coordinator according to the later, explicit coordination policy. Coordination does
not replace the Governor's desired-state authority. Its purpose is to make discovery,
DNS publication, record propagation and delegated placement observable.

A normal operation is:

1. The Governor records desired state and an authorization reference.
2. The relevant Maker retrieves the approved work for its own host or scope.
3. The Maker invokes PodMesh locally.
4. PodMesh returns a durable operation result and independent observations verify
   actual runtime effects.
5. The Maker publishes dated facts and evidence references to the control-services
   universe. DNS and registry publication follow verified facts, never a request
   acceptance alone.

## Operation during a partition

The objective is **continued bounded local operation**, not automatic global
authority without evidence. A partition must not make every host unusable, but it
also must not create two active owners of the same workload, address or exclusive
external effect.

Before a partition, allocate non-overlapping scopes. Examples include a host's
address-allocation pool, local disposable workload capacity, and explicitly
delegated placement work. A Maker may continue only inside its current scope.
It may not take over a remote workload replica merely because the remote host is
unreachable.

During a partition:

- each replica records local facts and operations with its own instance identity;
- names and DNS entries keep their established behavior; a stale record is visible
  as stale rather than silently asserted to be current;
- no automatic cross-host activation occurs without an explicit exclusion/fencing
  proof for the prior active instance;
- changes needing conflicting exclusive ownership are deferred or rejected;
- existing local workloads remain autonomous as far as their own data and network
  dependencies allow.

## Reconnection and priority

When connectivity returns, replicas first exchange their histories and compare
facts. Only then may the designated coordinator resume broader coordination. The
operator proposes a stable priority based on creation order, with the lowest
priority number coordinating after reconciliation.

Priority is a deterministic tie-breaker, not proof that one replica's data is newer
or that a remote workload stopped. It must never overwrite newer observed facts,
erase confirmed allocations, replay completed commands, renew revoked authority or
activate a duplicate workload.

Conflicting records require explicit resolution before an operation is issued. For
exclusive resources, safe choices are pre-delegated non-overlapping ownership,
verified handoff, fencing/exclusion, or human escalation. The system may remain
partially degraded while a conflict is unresolved; that is safer than a false
automatic repair.

## Relationship to migration, storage and backup

The control-services universe is not a substitute for the data required to restore
a workload. A recoverable universe needs, according to its declared scope:

- its container image by content digest;
- configuration and logical identity;
- persistent data or a coherent replicated recovery point;
- optionally, a compatible memory checkpoint for memory-preserving recovery;
- a verified decision about which instance may become active.

PodMesh migration transfers a stopped/checkpointed workload through a specific,
verified handoff. Future periodic checkpoints and backup storage can improve data
loss bounds, but neither alone implements automatic HA. DNS/name publication must
follow verified active placement; it must not itself direct traffic to a standby
replica before activation is authorized.

## Bootstrap and host reconstruction

A new or rebuilt host should not need a perfect existing cluster to join. It starts
with a host identity, a minimal configured trust/discovery seed, its Maker and local
PodMesh service. It discovers a reachable control-services replica by configured
endpoint, LAN discovery, WireGuard peer knowledge or another explicit bootstrap
source. It then receives only the records and scopes it is authorized to hold.

If all control-services replicas are absent, host-local PodMesh operations remain
available within their local authorization boundary. Recreating the logical service
requires a documented recovery/bootstrap procedure, evidence of the selected
recovery point, and an explicit decision about authority and stale allocations.
It must not invent a new global state from whichever host responds first.

## Implementation sequence for PodMesh

1. Keep the present local PodMesh API, migration contracts and durable evidence
   boundaries intact.
2. Define the registry schema: logical universe, instance, host, parent, name,
   address, scope, operation, evidence, freshness and conflict state.
3. Implement a single control-services universe locally, with explicit backup and
   recovery export/import, before replication.
4. Add replicas on the three laboratory hosts and prove record synchronization,
   DNS publication and bootstrap without creating a shared live filesystem.
5. Test host loss, partition and reconnection with pre-delegated scopes. Measure
   what remains available, what is deliberately blocked, recovery time and any data
   rollback.
6. Only then evaluate automatic takeover, resource fencing and a future HA contract.

## Non-claims

This document does not claim that replicated control services remove every need for
consensus, fencing or a human decision. It does not claim a partition-safe global
write protocol, instant failover, continuous memory replication, shared data
consistency, or replacement of a full virtual-machine platform. It specifies the
direction: simple local autonomy, explicit cross-host handoffs, replicated control
facts and recovery that stays possible when a host or control replica disappears.
