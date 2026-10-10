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
