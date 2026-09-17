# PodMesh Rust implementation plan

Status: planned, not implemented. Owner: Xavier de Poorter.
References: INTENT.md and the product requirements P01-P23 (maintainers' decision record).
Initial audience: our human-agent tandem. GUI follows the working CLI/API.

## Ordered implementation slices

| Step | Deliverable | Inventory | Acceptance |
|---|---|---|---|
| 1 | Rust workspace, podmeshd service and podmesh CLI; versioned configuration, structured errors, systemd lifecycle | P07,P18,P23 | Install, start, stop and restart on three hosts; configuration survives |
| 2 | Persistent host UUID, separate instance identity, local SQLite schema/migrations and operation journal | P02,P12,P23 | Restart preserves identity; cloning detected; incompatible operation-ID reuse rejected |
| 3 | Read-only Podman adapter: containers/images/runtime capabilities/resources; timestamped observations | P03,P08 | Compare results against actual Podman state on each host |
| 4 | Typed local API via Unix socket and CLI; authenticated identity and scoped authorization boundary | P06,P23 | Authorized requests accepted; unauthorized requests rejected; no free-text shell execution |
| 5 | Create/start/stop/logs/remove for explicitly managed disposable workloads; desired-version checks and recovery | P03,P05 | Effects observed; retries do not duplicate work; out-of-scope containers untouched |
| 6 | Maker integration using existing governor desired-state ledger | P15,P21 | Fictional SaaS requests child creation; maker invokes PodMesh and reports independently checked effects |
| 7 | Network/IP registry: selected /16, disjoint /24 grants, UUID/IP uniqueness and replica identity | P11,P12 | Collision/overlap rejection; pool owner and active workload host distinguished |
| 8 | Peer authentication, capability exchange and scoped metadata/event synchronization | P01,P13,P17 | Rejoin catches up; duplicate/out-of-order events handled; copied data grants no authority |
| 9 | Serial migration adapter around the proven experimental kit, persistent phases and explicit handoff | P04,P05 | Memory/identity continuity, checksum, source exclusion and interrupted-phase recovery |
| 10 | Periodic checkpoint replication with retention, coherent recovery metadata and operator-controlled recovery | P09,P10 | Last complete replica usable; stale/incomplete recovery point rejected; rollback measured |
| 11 | Existing-network routing and optional WireGuard for peer and bulk traffic; moving workload reachability | P11,P13 | Test both modes and same-origin-/24 peer traffic after a move; no duplicate IP advertisement |
| 12 | Manager bootstrap and scoped operation during partitions, deterministic reconnect coordination | P16,P17 | T01-T03,T07-T08; no takeover on unreachability alone; no lost confirmed allocation |
| 13 | Debian packages, reproducible release/build identity, upgrade/removal and host recovery guide | P07,P18,P19 | Real install/upgrade/removal/restore test; preserve state and identity deliberately |
| 14 | Three-host fractal exercise and operational report | P21,P22 | Governor-to-maker-to-PodMesh proof, resource/timing/failure evidence and human assessment |

Slices are incremental; package smoke tests start at step1 and deepen at step13.
Network-dependent placement/migration demonstrations wait for their network
prerequisites. Steps7-12 require design review before treating prototypes as HA.
Do not implement an alternative governor inside podmeshd.

## Proposed module boundaries

- contracts: typed requests, results, capability/version negotiation and errors.
- state: identity, schema migrations, operation journal and observed projections.
- podman: runtime adapter using supported APIs/structured arguments.
- operations: guarded lifecycle and resumable operation state machines.
- peers: authenticated transport and event exchange.
- network: address grants, routes and optional WireGuard integration.
- migration: checkpoints, artifact transport, validation and reconciliation.
- daemon/cli: system service and human/agent entry points over the same contracts.

Module names are proposals, not committed crate architecture. Do not store live
SQLite files in Git; final distributed register/authority design remains open.
Private keys stay in protected local storage, not replicated metadata.

## Dependencies and limits

The legacy vzcriu variant and workload-specific Podman reconciliation are
experimental dependencies. Rust orchestration does not generalize their support.
The current reconciliation identifies a specific demo process; production
reconciliation needs its own reviewed identity contract and tests.

Desired-state ledger authority, maker integration, supported Podman APIs/runtime
versions, IP prefix, ownership transfer and manager-replica semantics must be
resolved before their respective slice. Each slice records implementation,
executed evidence, deployment and operator acceptance separately.

DNS and human GUI are deferred. General database-volume consistency, transparent
external connection recovery and seamless failure takeover are not assumed.

## First runnable milestone

On three hosts: persistent distinct identities, read-only Podman inventory,
local CLI/API and durable operation status. Then one authorized lifecycle operation
through a maker. No prerequisite GUI, global scheduler or automatic HA.

All steps currently TODO. This file is an implementation inventory, not an
announcement that a Rust service already exists.

## Required deployment matrix

Operator clarification: support BOTH standalone Linux-host operation and execution
inside a SHAPER OS universe. Add P24 to every relevant slice's acceptance matrix.

- Keep core contracts independent of SHAPER dependencies.
- Implement standalone service configuration, logs and health/status surfaces.
- Add SHAPER adapters that reuse existing Logger and supervision contracts.
- Package direct-host and containerized entry points using the same core.
- Specify runtime access, credentials, mount paths and required capabilities;
  do not assume a containerized daemon can manage the outer host automatically.
- Exercise inventory, authorized actions, restart/recovery, logging and migration
  separately in each supported mode, recording unsupported combinations explicitly.

The first delivery may progress incrementally, but host-only success does not
satisfy the complete dual-mode requirement.
