# PodMesh documentation

This repository carries what is needed to understand, build, qualify and operate PodMesh. The maintainers keep the laboratory's status, campaign
records, reviews and decisions in a private companion repository; nothing here depends on it.

## Direction
- [../INTENT.md](../INTENT.md) — the purpose, actors, the human-agent contract and the delivery discipline.
- [IDEAL-SCENE.md](IDEAL-SCENE.md) — what PodMesh ought to be: goal, purposes, the invariants, the valuable final
  products, the ideal scene per area and per topology (one host to ten and more, with or without shared storage).
- [IDEAL-SCENE-PROGRAM.md](IDEAL-SCENE-PROGRAM.md) — the targets that reach the ideal scene, in order.
- [IDEAL-SCENE-CHECKLISTS.md](IDEAL-SCENE-CHECKLISTS.md) — the certification criteria that prove each part.
- [EXPERIMENTAL-SCOPE.md](EXPERIMENTAL-SCOPE.md) — the research scope and the review contract.

## Operate
- [PREPARE-A-HOST.md](PREPARE-A-HOST.md) — the validated host target, installation and storage recommendations.
- [OPERATIONS.md](OPERATIONS.md) — the experimental tools: moving, replicating, guarding and taking over universes.
- [LOCAL-API.md](LOCAL-API.md) — every operation of the host's local API, its fields, refusals and replay rules.

## Contracts and designs
- [UNIVERSE-HIGH-AVAILABILITY.md](UNIVERSE-HIGH-AVAILABILITY.md) — activation leases, epochs, fencing, recovery points,
  live replication, takeover and continuity of service.
- [MIGRATION-PROTOCOL.md](MIGRATION-PROTOCOL.md) and [MIGRATION-INTEGRATION.md](MIGRATION-INTEGRATION.md) — transfer
  authority, source exclusion, recovery, and the boundaries of the migration chain.
- [UNIVERSE-NETWORK-CONTRACT.md](UNIVERSE-NETWORK-CONTRACT.md) and [NETWORK-AND-PLACEMENT.md](NETWORK-AND-PLACEMENT.md)
  — the universe network and placement decisions.
- [CONTROL-SERVICES-UNIVERSE.md](CONTROL-SERVICES-UNIVERSE.md) — the replicated manager: identities, registry,
  replication, partition, reconnection and host reconstruction.
- [MANAGER-ADMINISTRATION.md](MANAGER-ADMINISTRATION.md), [MANAGER-PUBLISHER-CONTRACT.md](MANAGER-PUBLISHER-CONTRACT.md)
  and [PUBLISHER-FOLLOW-LAB.md](PUBLISHER-FOLLOW-LAB.md) — the manager's administration surface and the public entry
  point that follows the governor.
- [GARBAGE-COLLECTION.md](GARBAGE-COLLECTION.md) — collection on proof, never on age alone.
- [BACKUP-SERVER.md](BACKUP-SERVER.md) — the Backup Server design (recovery points, immutable chunks, off-site copy).
- [SHAPER-SUPERVISION.md](SHAPER-SUPERVISION.md) — integration with SHAPER's supervision loop.

## Engineering
- [RUST-IMPLEMENTATION-PLAN.md](RUST-IMPLEMENTATION-PLAN.md) — the implementation plan.
- [ACCEPTANCE-TEST-PLAN.md](ACCEPTANCE-TEST-PLAN.md) — the acceptance gates and their required evidence.
- [../AGENTS.md](../AGENTS.md) — what belongs in this repository, in the private companion, and in neither.
