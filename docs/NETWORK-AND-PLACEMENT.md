# PodMesh network and placement decisions

Status: operator design decisions, 2026-09-11. Not implementation or canon.

The consolidated decision record is kept by the maintainers; read [IDEAL-SCENE.md](IDEAL-SCENE.md) for the direction and later manager/partition
decisions. Storage engines and distributed DNS serving below are early proposals,
not selected implementations. The later manager proposal includes the DNS service.

## Current network direction

- One logical IPv4 /16, divided into distinct /24 allocation pools per host.
- Workload UUID is stable; its assigned IP survives a host move.
- Unique (network UUID, IP) mapping to workload UUID. Replicas share workload
  identity; their instance identity, host and active/standby role are separate.
- Initial proposal: per-host SQLite metadata exchanges versioned events, not database files. Final storage selection remains open.
- Disconnected allocation requires non-overlapping, previously assigned pools.
  Pool ownership conflicts on joining independently formed meshes must be resolved
  before activation; event replication alone does not guarantee exclusivity.
- Connectivity may use existing IP routing or optional WireGuard. Checkpoint,
  image and data transfers may use WireGuard as well as control traffic.
- Mobile workload routes and exclusive activation must be coordinated. Routing
  an address to a new host does not itself revoke the previous instance.
- IPv4 prefix selection, allocator, schema and network implementation remain open.

## Deferred: shared DNS and fractal placement

The operator's example has three logical universe levels:
SaaS universe -> shop-manager universe -> shop universe.
These are deployment examples, not a redefinition of the OS/Runtime/Workspace
layers. A logical parent-child relationship must not require co-location.
Universes may be placed on different hosts or sites according to resources,
compatibility and explicitly authorized placement policies. Connectivity can
traverse WireGuard when needed; WireGuard remains optional.

The registry must distinguish:
- stable universe identity and logical parent;
- current hosting location and active instance;
- network identity and reachable endpoint;
- observed health, observation time and observer identity;
- authorized supervision and intervention scope.

A parent can discover and check its children despite placement changes. Loss of
reachability is not proof that a child stopped. Registry knowledge is not an
intervention grant; existing supervision contracts still govern authority.

A common DNS namespace, served redundantly by hosts, could resolve universe names
from the UUID/IP registry. Duplicate names, stale observations, partitions and
client resolver discovery require design and tests. DNS is explicitly deferred.

Existing reference checked: SHAPER-OS-V1.14/docs/architecture/UNIVERSE-PROFILES.md
(+parent supervisor of other universes); docs/architecture/BRICKS.md describes
pkg-supervisor ingesting child vitals. These references describe the existing
architecture; this note does not modify its governing rules.
