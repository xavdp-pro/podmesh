# Bounded PodMesh host adapter

Status: implementation contract for the next candidate increment. The existing
root-host daemon has not established this boundary. This contract supplements
[FUNCTIONAL-UNITS.md](FUNCTIONAL-UNITS.md); it does not replace the target universe,
nested or migration capabilities with a flat-only product.

## Trust and authority

The private node application is `podmesh-node`, a non-login UID/GID above 1000,
with its own private database and volumes. Its only host crossing is a dedicated
Unix socket to a root host capability provider. No Podman socket, host filesystem
mount, unrestricted command, shell or root credentials enter the application.

A root-owned 0600 host policy declares the authorized application peer UID,
node/scope identity, allowed operation families, local immutable image IDs,
resource ceilings and exact optional host mounts. Unix peer credentials and the
policy are checked independently of application request content. A socket path,
caller-supplied `authorization_ref`, majority or UID alone grants no governance
power. The human/parent standing mandate grants the provider its capability scope;
the application checks the operation mandate and journal ownership within it.

`allowed_operations` is required and must be an array of known lifecycle names.
An explicit `allowed_operations: []` grants zero lifecycle operations; omission,
null, non-array values, non-text entries and unknown names still refuse. Other
policy requirements, including protected policy/state paths, peer identity,
images, grants and resource ceilings, remain mandatory. No dummy operation or
`host_identity` operation token is needed to obtain the existing host fact.

With an empty list every effect (including clone, legacy/contained nested create
and snapshot cleanup) refuses before engine dispatch or intent/ownership writes.
Legacy `inventory`, `inspect`, `cgroup_limits` and `recovery_bindings` requests
also refuse: their ownership observation can pin a container identity, and a
stop-bound inspect can persist an intent. They are not read-only authority.
Existing non-repairing fact paths retain their original validation and scope;
no new fact, lifecycle capability or recovery/migration effect is granted.
Nonempty policies keep their existing operation gates and semantics.

Before each effect the application commits canonical request/operation/attempt
intent in its own store. The request carries that operation identity, request
binding and mandate reference. The adapter validates the typed effect against
that operation and its host-owned policy; the application owns the journal and
records the observed result after the effect. The application is trusted to attest
its committed intent; the host does not acquire database credentials. Unreachable
adapter or unknown intent never invokes a local rootful Podman fallback.

## Protocol and effects

Protocol version: `podmesh-host-capability/1`. One bounded JSON request and response
per connection. Unknown fields, actions, versions, oversized values and non-finite
resource limits refuse. A response reports command outcome and observation; an
acknowledgement is not the application's verified operation result.

| Typed capability | Inputs | Host enforcement |
| --- | --- | --- |
| Inventory / inspect | Declared scope, optional universe UUID | Return only scope-owned containers; no caller-controlled filters or paths |
| Image inventory | Scope | Whitelisted local image IDs and scope-owned snapshots; never pull |
| Create | Operation identity, UUID, image ID, command vector, declared profile | Fixed names/labels; local pinned image; fixed isolated network for first slice; no caller flags |
| Start / pause / resume / delete | Bound UUID | Adapter-owned container ID and scope checked again; delete stopped only, no force or volume removal |
| Stop | UUID, bounded timeout, declared escalation | Same ownership; signal from inspected container; bounded process group; escalation explicit |
| Resources | UUID, memory/CPU limits | Policy ceilings and host capacity; fresh host cgroup observation for same inspected container |
| Clone snapshot / create | Source and destination UUID, operation ID | Stopped mount-free owned source, adapter-owned snapshot tag/provenance; source identity rechecked |
| Snapshot cleanup | Scope-owned snapshot identity | No force; no arbitrary image removal, retained dependants reported |
| Host identity/capacity/storage | Scope, owned universe if needed | Host facts from provider namespaces, not application `/proc` or `/sys`; no requested file paths |

Effect retries continue through existing journal lifecycle ordering and re-observe
host identity/state; the adapter never blindly repeats a saved action. An adapter
ownership record binds container ID and creation operation across its restart.
A conflicting name/identity refuses rather than adopting an existing container.
Failure between effect and observed journal result remains recoverable through
that operation's original identity. Adapter state and private policy belong in
coherent recovery evidence alongside application state.

## Incremental scope and qualification

The first executable transport may support flat isolated universes and the eight
existing lifecycle operations. Nested, managed-network, secret and host-state
mount actions must refuse explicitly until their distinct capability is ported.
These refusals protect an increment; they do not complete the product candidate.

The required nested capability names an exact host-owned image/UUID/mount allowlist
and namespace/device capability envelope. It never accepts arbitrary `--privileged`,
mount flags or caller-selected host paths. Qualify the intended nested service and
its declared disk/log recovery separately; inner RAM continuity is not implied.

Build/tests use an immutable committed source pin and dedicated host fixtures.
Verify peer refusal, unknown/type/path/flag rejection, scope/image/resource bounds,
operation binding, pending-before-effect, outcome-after-observation, interrupted
retry, container replacement, stopped clone/delete and actual eight-operation
lifecycle. Then qualify exact exported bytes in the private node pod with its own
DB, application UID, provider and persistent recovery state. A protocol parser test
or an adapter that refuses every real effect does not close this boundary.

## Containment of a nested outer universe

A rootful outer container that holds `SYS_ADMIN` and a writable `/proc/sys` is equivalent to root on
the host: it can rewrite init-namespace sysctls such as `kernel.core_pattern` or `kernel.modprobe`,
whose helpers run with host privileges, and it can remount what was made read-only. This is true with
`--privileged` and with a narrowed capability list alike. Making `/proc/sys` read-only is not a
remedy: the nested network stack (netavark) must write the network sysctls of the namespaces it
creates.

The contained source shape is a **user-namespaced outer**, selected by the distinct
`allowed_contained_nested` grant and `network_profile: host-policy` create request.
Its profile is `userns-v1`; it never falls back to a legacy privileged outer.
The legacy `allowed_nested` / `privileged-host-device-access` path remains a separately
labelled host-root-equivalent laboratory increment. Historical standalone qualification
does not qualify this new provider source or a package built from it.

- the outer's root maps to an unprivileged host UID/GID range (for example 65536 IDs that no other
  container uses), so its capabilities act only on namespaces the outer owns;
- host directories given to the outer (configuration, unit state, inner image store, test fixtures)
  are idmapped mounts, so files keep their owners inside and stay unprivileged on the host;
- the capability set is limited to what nested Podman needs: `SYS_ADMIN`, `NET_ADMIN`, `NET_RAW`,
  `MKNOD`, `SYS_RESOURCE`, `SYS_PTRACE`, `SETFCAP`, `AUDIT_WRITE`, with `/dev/fuse` and `/dev/net/tun`;
- `/proc/sys` is writable for the outer's own network namespaces and `net.ipv4.ip_forward=1` is set;
  writes to init-namespace sysctls are refused by the kernel;
- the inner store uses kernel overlay on a dedicated mount, and the outer's default nested network
  uses a subnet distinct from the host's container network.

Qualification must show both sides: a write to `kernel.core_pattern` from the outer is refused and
the outer's processes run under the mapped host UIDs, **and** every functional unit inside still
passes its real effects, failures, recovery and restore. The `privileged-host-device-access` grant
above names the host-root-equivalent envelope. Production containment needs a distinct grant naming the
user-namespaced envelope (UID/GID mapping, idmapped mounts, bounded capabilities,
observed effective namespace) and qualification on that provider path.

### Contained grant and recovery handoff

The host policy's `allowed_contained_nested` array defaults to empty. Each entry has
exact fields: `universe_uuid`, immutable local `image`, `containment: userns-v1`,
`uid_start`, `gid_start`, `id_count`, `volume_root`, `mounts`, and `network`.
The mapping is a single range from container ID zero, excludes host IDs below 65536,
and covers 65536 through 1048576 identities. Declared UID and GID ranges and volume
roots cannot overlap other contained grants; a UUID cannot also have a legacy grant.
Before create, existing private container mappings in the provider's configured Podman store are inspected for conflicts. The host constructor allocates unused ranges across any other independently managed stores; store-local inventory is not a host-wide allocator.

Each mount has `source`, `destination`, and boolean `read_only`. Sources are existing
protected root-owned directories beneath the grant's dedicated `volume_root`; no
symlink, overlapping source/destination or host-control destination is accepted.
The provider supplies `idmap,rprivate` and the declared access. It never recursively
changes ownership. Typical generic destinations are `/etc/shaper`, `/var/lib/shaper`
and `/var/lib/containers`; source paths and mapping ranges are deployment inputs.
No application request supplies a host path, namespace mapping, capability or device.

`network` is `none` or one existing bridge network whose name matches exactly and
whose `io.podmesh.adapter-scope` label matches the host policy scope. It is not a
host namespace or an implicit default network. The constructor provides its DNS,
route and outbound connectivity when required; a successful provider start alone
does not prove that connectivity. The create operation explicitly selects
`network_profile: host-policy`, so a routed bridge is never reported as the
legacy `isolated` / no-network profile.

The provider fixes the capability/device envelope, UID/GID mappings, private
PID/IPC/UTS/network/cgroup namespaces and idmapped mounts. Its ownership record
binds the complete grant and each source directory's device/inode. Replacing a
directory or changing the policy does not adopt existing runtime; an explicit new
construction/recovery binding is required. Configuration is checked from actual
Podman inspect, not the create-command text. Running or paused instances additionally
prove kernel UID/GID maps, mapped process identities, separate namespaces and
idmapped source/view ownership through `/proc`. `PodMeshContainment` reports
`configuration_verified` and `runtime_verified` separately, including the selected
network. Unsupported Podman, kernel or filesystem behavior refuses; no privileged
fallback, volume-copy imitation or silent privilege widening is available.

The node read operation `universe_recovery_bindings` requires the exact owned
`universe_uuid`. It returns `podmesh-contained-recovery-bindings/1`, the current
container/image identity, retained grant/source binding, every declared mount and
the actual stopped/running state. This is the full-volume recovery handoff to the
authorized SHAPER constructor: cleanly stop the source, capture consistent private
SQL dumps and all declared non-reproducible volumes, retain keys separately, restore
into an empty target under a distinct contained grant and prove useful operation.
The provider does not manufacture a backup or parent Logger receipt. Rootfs-only
snapshot/clone is explicitly refused for contained instances: such an image omits
the declared persistent volumes. Existing mount-free legacy clone semantics remain.

Engineering checks use the native Rust suite. The explicitly armed real effect
test `real_contained_nested_provider_when_explicitly_named` reads
`PODMESH_HOST_CONTAINED_FIXTURE`, a JSON object with `scope`, existing empty root0700
`state_dir`, the exact `grant`, and an explicit `command`. It exercises real create,
start, original-create retry while running, pause/resume, stop, delete and recovery
binding observation. The fixture owner supplies an already-local image, unused
mapping ranges, protected directories and optional scope-owned network. Unarmed
tests are not runtime evidence; even an armed engineering fixture is not the full
private-node/MariaDB/SHAPER nested service and restoration qualification.

## Implemented transport increment, pending qualification

The node source now has a separate provider and application wrapper. Host boot
facts are read once per pass and validated. The provider's optional `allowed_nested`
grants bind exact UUID/base image, an empty host-mount set, private PID/IPC/UTS and
network-none, and explicit `privileged-host-device-access` for the Rule11 outer
universe. This names the broad privileged device envelope honestly; it does not
claim a fine-grained device allowlist. Nested cloning preserves the profile and
requires a distinct target grant for the same base image. No inner workload or
continuity proof is supplied by this source increment. Arbitrary mount/device or
namespace flags remain rejected; managed network, secrets and host-state mounts,
migration/replication effects and imported lease/epoch boot gates remain unported.
The initial flat-only increment above is historical scope, not the product target.

### Observation while an owned stop is completing

The provider verifies the complete contained runtime before sending a stop signal.
A successful dispatch then records an atomic root-owned receipt bound to the
immutable stop intent, container ID, start timestamp and observed host PID.
Inspection inside that same committed stop may observe shutdown without requiring
live volume mount ownership: systemd can unmount those volumes before PID 1 exits.
This exception lasts only the declared timeout plus the 30-second command margin;
a missing, failed, foreign, expired or restarted-generation receipt retains the
complete verifier. Static grant, source identity, container ownership, configuration
and actual OCI idmaps remain mandatory. The shutdown view explicitly reports
`runtime_verified=false`, with `runtime_observation=owned-stop-in-progress; live-mount-proof-not-claimed`.
Ordinary inspection, inventory and recovery queries always require the full live
proof. The application still waits for process exit and rejects a changed start
timestamp; a successful signal receipt alone never proves a completed stop.
