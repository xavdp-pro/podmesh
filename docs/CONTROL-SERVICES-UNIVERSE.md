# Control-services universe: complete implementation brief

Date: 2026-09-12. Audience: PodMesh implementation and review agents.
Owner: Xavier de Poorter, collaborating with Codex and Claude Code.
Status: design direction and implementation brief. It describes the target and
the decisions still needed; it is not evidence that the target already works.

## 1. Why this component exists

PodMesh is intended to make a group of autonomous Linux hosts easier to use than
a traditional tightly coupled virtualization cluster. A host must be able to join
with existing work, leave, fail, be rebuilt, and return without making every other
host unusable or requiring the operator to evacuate all workloads first.

The operator's experience with clustered virtualization is that shared storage and
global quorum are useful but can make recovery difficult. A lost quorum, a broken
shared filesystem, or a host that cannot be cleanly rejoined may turn a local
problem into a cluster-wide recovery operation.

PodMesh does not attempt to deny those engineering problems. It changes the
dependency order:

- local runtime operation must remain possible on each healthy host;
- control facts are replicated rather than stored in one shared live filesystem;
- a host never treats loss of reachability as proof that another host stopped;
- exclusive activation, migration and resource ownership remain explicit;
- unresolved conflicts may keep a limited part of the system degraded rather than
  performing a false automatic recovery.

The target is practical freedom with evidence: connect hosts, place workloads,
move them, rebuild a host, and recover control services without every action being
dependent on one central cluster state.

## 2. The logical component

The older discussion name was “manager”. New technical material should call it the
**control-services universe**. It is one logical universe replicated on every host.

It is a Podman-hosted package of control services, initially intended to provide:

- the universe registry;
- naming and DNS publication;
- host and replica discovery;
- dated observations and evidence references;
- synchronization of control records;
- a later status/control interface for the human-agent tandem.

It is not a universal administrator. Replication gives availability of control
services; it does not grant host-root access, policy authority, or permission to
activate workloads.

## 3. Roles and authority

| Component | Owns | Does not own |
| --- | --- | --- |
| Human / Steward | purpose, scope, authority, exceptional decisions | routine execution of every safe operation |
| SaaS Governor | desired state, delegation, coordination, escalation | direct host-root commands or unrestricted host keys |
| Maker on each host | approved local work, local evidence publication | desired-state policy or cross-host takeover by assumption |
| PodMesh on each host | Podman lifecycle, checkpoint/migration mechanics, durable operation evidence | global scheduling or a second desired-state ledger |
| Control-services universe | registry, DNS, discovery, replication of control facts | Governor, Maker, or implicit authority |

Normal flow:

```text
Human intent
  -> Governor records desired state and authorization reference
  -> Maker retrieves work for its host and scope
  -> PodMesh performs a typed local operation
  -> Independent observation verifies the actual result
  -> Maker publishes dated facts and evidence to control services
  -> Registry/DNS publish only verified placement facts
```

No participant obtains extra authority from an informal sentence, a replicated
database row, or a successful health endpoint.

## 4. Identity model

The design must distinguish logical objects from the running copies that currently
host them.

| Identity | Meaning | Stability |
| --- | --- | --- |
| Universe UUID | logical workload identity | permanent across clone/migration/recovery where appropriate |
| Universe instance UUID | a particular active or standby runtime copy | new for each created/restored replica |
| Host UUID | stable host identity | preserved through ordinary restarts; replacement is explicit |
| Control-service logical UUID | one logical replicated service | same across all replicas |
| Control-service instance UUID | one running replica on one host | distinct per host/creation |
| Operation ID | idempotent request identity on the acting host | immutable audit identity |
| Authorization reference | provenance from Governor/Human | not yet a cryptographic remote credential |

For a workload, logical identity, IP/name identity, current host, active/standby
role and runtime container ID are separate fields. A restored container may have a
new runtime ID while remaining the same logical universe. An old runtime ID must not
regain ownership merely because its labels still exist.

## 5. Registry: what it records

The registry is control data, not a universal filesystem and not an application
database for every workload. Each record needs provenance, freshness and a conflict
state.

Minimum proposed record classes:

| Record | Minimum contents |
| --- | --- |
| Host | host UUID, instance facts, reachable endpoints, capacity observations, last observer/time, supported PodMesh capabilities |
| Universe | universe UUID, parent UUID, declared kind, desired state reference, current placement facts, image digest, lifecycle state |
| Instance | instance UUID, universe UUID, host UUID, Podman container ID, active/standby role, created/restored operation binding |
| Network assignment | network UUID, IP, DNS names, gateway/prefix, assigned scope, active route owner, evidence/time |
| Delegated scope | issuer, recipient Maker/host, scope type, allocation range or workload list, validity window, revocation status |
| Operation fact | operation ID, host, actor, authorization reference, requested effect, verified outcome, evidence reference/time |
| Migration handoff | source/destination, logical universe, artifact hashes, claim/outcome references, source exclusion state |
| Conflict | competing records, affected exclusive resource, detection time, automatic action allowed, required resolver |

All facts must identify who observed or wrote them and when. “Unknown” and “stale”
are real states; absence of a fresh observation is never proof of health or failure.

## 6. Storage choice: the initial direction and decision required

The recommended first implementation is **SQLite per control-service replica plus
an append-only, versioned event/history log**. The replicas exchange records or
events, never live SQLite files.

Why this is a sensible first step:

- the control dataset is initially small: identities, mappings, operations,
  observations and scopes;
- SQLite is simple, local, durable, inspectable and easy to back up;
- each host keeps working with its own last known records during an interruption;
- merge and conflict rules remain visible in the application instead of being
  hidden inside an apparently shared filesystem;
- it avoids introducing a distributed database or global quorum before the actual
  consistency requirements have been measured.

This is an implementation proposal, not yet an approved storage standard. Before
coding replication, record the decision explicitly:

1. SQLite plus event log is accepted for the first laboratory implementation; or
2. another engine is selected, with its partition/recovery behavior documented.

Do not treat a Git repository, a shared directory, or copied database files as the
replication protocol. Git can preserve design and source history; it is not the
runtime arbiter for exclusive resource operations.

## 7. Replication contract

Each control-service replica has a local durable store and an outbound/inbound
history cursor. When peers are reachable, they exchange missing immutable events and
materialized facts. The protocol must preserve the original event identity and must
not create a new operation merely because a fact is received again.

Every event requires at least:

- globally unique event ID;
- producing control-service instance UUID and host UUID;
- sequence number local to that producer;
- wall-clock observation time and receive time;
- record type and version;
- payload hash;
- causal predecessor or vector/version relation sufficient to detect a conflict;
- evidence reference when the event claims an external effect.

Replication may automatically merge only facts that are demonstrably non-exclusive,
such as independent observations, immutable operation results and non-overlapping
allocations. It must not automatically merge contradictory active-instance claims,
IP assignments, host scope ownership, revocations, or completed external effects.

When a conflict is found, create a conflict record, prevent the conflicting action,
retain both histories, and escalate according to the delegated authority. A later
coordinator may propose a resolution; it must not silently erase the losing history.

## 8. Connected operation

When replicas communicate normally:

1. They synchronize missing records and verify their hashes.
2. They materialize the same logical registry view when no conflict exists.
3. They select a coordinator according to the explicit later priority policy.
4. The coordinator assists discovery, DNS publication and routing of requests; it
   does not replace the Governor's desired-state authority.
5. Makers remain the only actors that invoke the local PodMesh API for host work.
6. DNS publication follows verified active placement, not a requested placement.

The initial implementation must expose replication health: peers seen, last common
event, lag, stale records, blocked conflicts and bootstrap status.

## 9. Network model

The current direction is one logical IPv4 `/16`, divided into non-overlapping `/24`
allocation pools per host. A universe has a stable UUID and a unique assigned IP in
its logical network. An IP assignment is distinct from the host that currently
routes it.

Hosts may communicate on their existing network or through optional WireGuard.
WireGuard can protect control traffic and transfer checkpoint/image/data artifacts,
but PodMesh must work without making it mandatory. A transport is not an authority
mechanism: it carries authenticated requests and records; it does not decide which
workload may activate.

Initial DNS direction:

- names resolve from verified UUID/IP/placement registry records;
- replicated control-service instances may serve or publish DNS;
- a moved universe keeps its logical name; the route/active placement changes below
  it only after verified handoff;
- stale DNS or registry knowledge is visible and bounded by a chosen TTL/refresh
  policy, not silently assumed correct.

Actual prefix, DNS zone, routing implementation, TTL policy and WireGuard identity
mechanism remain decisions to make before network implementation.

## 10. Partition model

The objective is to avoid a global quorum dependency that blocks all healthy hosts.
It is not a claim that all writes are safe everywhere during a split.

Before a partition, the system assigns non-overlapping rights. Examples:

- a `/24` address allocation pool owned by one host;
- explicitly assigned disposable workload capacity;
- a set of universes a Maker may maintain locally;
- a migration that carries a verified transfer handoff;
- a read-only observation/synchronization task.

During a partition, a host may continue inside its known valid scope. It must not:

- allocate another host's address range;
- activate a remote standby merely because the primary is unreachable;
- overwrite a remote ownership record;
- issue a conflicting exclusive placement;
- convert missing observations into a “stopped” verdict.

Existing local workloads continue as far as their own dependencies permit. The
control-services replica stores facts for later synchronization. It may be locally
useful even when it cannot coordinate globally.

## 11. Reconnection and coordinator priority

When connectivity returns, the sequence is mandatory:

1. Discover the replicas and authenticate their identities.
2. Exchange histories until each peer can state what it lacks.
3. Verify record hashes, sequence continuity and evidence references.
4. Materialize non-conflicting records.
5. Create conflict records for exclusive conflicts.
6. Select the coordinator only after the preceding reconciliation.
7. Resume broader coordination only for facts that are conflict-free and within
   authority.

The operator proposes a stable priority derived from creation order: the lowest
priority number coordinates after reconciliation. This is a deterministic
tie-breaker, not a truth oracle. It does not prove that the lower-number replica
has newer records, that a remote workload stopped, or that a historic command may
be replayed.

The exact priority field, tie-break rule, failure detector and permitted coordinator
actions remain to be specified and tested. Do not implement priority as automatic
global takeover.

## 12. Migration, backups and workload data

Replicating control records is not enough to restore a workload. A recovery plan
must independently name:

- image digest and availability policy;
- universe configuration and logical identity;
- persistent volumes or an application-consistent data recovery point;
- optional compatible memory checkpoint;
- source exclusion and active-instance authorization;
- network route/name transition;
- expected data-loss and recovery-time bounds.

The present PodMesh serial migration is a bounded, experimental two-host API
capability for one workload shape. It is a foundation for explicit handoff, not
proof of generic live migration, replicated memory or automatic HA.

PodMesh Backup Server is a separate later service. It stores versioned recovery
points; it must not be confused with replicated control services or a running
workload replica. Deletion, corruption or stale state may propagate through
replication, so backup retention and restore verification remain separate contracts.

## 13. Bootstrap and host reconstruction

A host must be reconstructible without demanding a perfectly healthy cluster.
Initial bootstrap artifacts should be small, explicit and recoverable:

- host UUID or explicitly documented replacement identity;
- local Maker and PodMesh installation;
- control-service logical identity and trusted discovery seed;
- one or more discovery paths: configured peer endpoint, local-network discovery,
  WireGuard peer data, or an explicit operator bootstrap source;
- local credentials/certificates or keys according to the chosen transport design;
- current scope allocation and a known recovery-point reference.

The bootstrap seed must not be hosted only behind the DNS service it is supposed to
recover. A rebuilt host receives records only after authentication and only for
scopes it is allowed to use.

If every control-service replica is absent, local PodMesh remains usable for its
local operations. Recreating the control plane requires an explicit recovery
procedure: select an evidence-qualified recovery point, establish new replica
identities, handle stale allocations and authority, and record the human decision.
The system must not elect a new global truth merely because one disconnected host
answers first.

## 14. Required design decisions before implementation

Claude should not invent these values. Record each decision, its owner and its
acceptance test before using it in a production-shaped implementation.

| Decision | Initial recommendation | Still required |
| --- | --- | --- |
| Local registry store | SQLite plus append-only event history | operator acceptance and schema |
| Event ordering/conflict detection | producer sequence plus causal/version relation | exact format and merge rules |
| Replication transport | direct existing network, optional WireGuard | authentication, encryption and peer enrollment |
| DNS mechanism | replicated control service publishes verified records | server choice, zone, TTL and client discovery |
| Coordinator priority | immutable creation-order priority, lowest value after reconciliation | tie-break, failure detector and allowable actions |
| Partition write rights | pre-delegated non-overlapping scopes | scope vocabulary, validity and revocation |
| Exclusive conflict resolution | reject/defer, verified handoff/fencing, or human escalation | resolver procedure and evidence format |
| Bootstrap | minimal seed outside the service DNS dependency | storage, rotation, recovery and trust policy |
| Workload data recovery | separate image/data/checkpoint contracts | first supported volume/database and backup target |

## 15. Implementation and test order

1. Finish and independently review the current bounded migration/recovery work.
2. Define and commit the registry schema and an immutable event format.
3. Build one local control-services universe with registry read/write, explicit
   export and import, and no replication yet.
4. Test restart and recovery of that one instance from its own backup.
5. Replicate it to two laboratory hosts. Prove idempotent event transfer, history
   catch-up, stale-record visibility and conflict creation.
6. Add the third host. Prove bootstrap from each host and control-service loss on
   one host without losing local PodMesh operation on the others.
7. Add DNS publication from verified registry facts and test a universe move.
8. Test a partition, allowed local work inside pre-delegated scopes, reconnection,
   history reconciliation and conflict handling.
9. Only after those tests evaluate controlled takeover, fencing, automatic recovery
   and a future HA claim.

Each stage must produce raw evidence, exact versions, an independent observation,
recovery steps and a list of limitations. A healthy endpoint alone is not proof.

## 16. Explicit non-claims

This direction does not claim:

- that a shared filesystem is never useful;
- that replication eliminates every consistency or consensus problem;
- that priority alone solves split brain;
- instant failover, zero data loss or continuous memory replication;
- a production-ready DNS, registry, backup service or HA system;
- generic migration for networks, volumes, arbitrary runtimes or all Linux hosts;
- authority created by a replicated service, UUID, DNS record or model decision.

The intended result is smaller, more explicit and more repairable: local autonomy,
replicated control facts, explicit handoffs, independent recovery paths and proof at
each stage.
