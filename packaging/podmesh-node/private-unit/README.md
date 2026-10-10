# Private node application and host capability provider

Status: executable candidate increment; qualification pending. This supplies the
application wrapper and the separate `podmesh-host-adapter` binary's inputs. It is
not a full installer, a host cutover or completion of the product target.
[HOST-ADAPTER-CONTRACT.md](../../../docs/HOST-ADAPTER-CONTRACT.md) owns the boundary.
The [immutable delivery recipe](../delivery/README.md) packages those exact
candidate bytes with an explicit new-instance install/start/stop/rollback
controller. Package construction and runtime qualification are separate gates.

The application is `podmesh-node`, non-login UID/GID 1102, inside a dedicated private
Podman pod with its own MariaDB server and separate owned application/DB volumes.
The scoped DB user and database are `podmesh-node`. The server keeps its upstream
identity. Never reuse the manager's pod, database, credentials or volumes. Publish
no node/database port: the node API and host crossing are dedicated Unix sockets.

## Immutable construction

On the declared build host, copy the manifest-selected MariaDB-feature `podmeshd`
into a private clean context with this Containerfile. Build with `--network=none
--platform linux/amd64 --pull=never`, explicit `NODE_BINARY_SHA256`,
`BINARY_SOURCE_REVISION` and `RECIPE_REVISION`. All are required; the recipe verifies
the copied bytes and creates the real account. Export the resulting OCI image and
record its ID/digest/archive hash plus the extracted executable hash. Separately
build `podmesh-host-adapter` from that exact source pin, and record its executable
hash in the same candidate manifest. Runtime loads those exact bytes.

## Host-owned policy and adapter state

Prepare only new declared candidate paths. Copy `host-policy.example.json`, replace
scope/path and explicitly allow local full image digests. An empty image list grants
no create capability. The policy file must be regular, root-owned 0600, and adapter
state an existing absolute root-owned 0700 directory. Each created universe has an
fsynced record binding creation operation, action digest and observed container ID.
The policy ceilings must fit actual host capacity. Creates apply those ceilings
and report their observed values; ceilings are not reserved consumption. The
finite `max_universes` budget counts retained identity records, including deleted
universes; history is not discarded to free a budget. Canonical request hashes
also bind up to 10,000 retained operation IDs; a changed request under an old ID
refuses before effect. Identity reuse needs a separate
explicit recovery/adoption path, not a new create with an old UUID.

Prepare a new root-owned, application-GID-owned socket directory, mode 0750; the
adapter requires that numeric GID and refuses writable/shared or existing endpoints. A restart after abrupt termination requires the runtime owner
to verify and remove only its stale socket endpoint, or choose a new declared
endpoint while preserving the provider state directory; the provider never
unlinks an existing endpoint automatically.
For example, after verifying the explicitly named paths are unused:

```sh
# Run under the separate authorized host administration path.
podmesh-host-adapter --policy "$HOST_POLICY" --socket "$HOST_SOCKET_DIR/capability.sock"
```

The provider runs as root, outside the application and without a client socket
environment variable. It checks kernel peer UID against policy before parsing
commands. The application verifies the server peer is root. Mount only this new
socket directory read-only at `/run/podmesh-host`; do not mount the Podman socket,
host root, `/proc`, `/sys` or the adapter's policy/state into the application.

## Private unit inputs and execution

Use a dedicated private bridge/pod as for the manager recipe, with no published
ports. The private MariaDB endpoint is `127.0.0.1:3306` in the shared pod namespace.
Mount a read-only 0600 profile/password configuration at `/etc/podmesh-node`, a
persistent application volume at `/var/lib/podmesh-node`, and a private writable
runtime directory at `/run/podmesh-node` (tmpfs UID/GID1102 mode0700). The application
volume must be numerically owned by 1102; configure only new volumes, never chown
existing host state. Start the host provider and DB first, verify private DB access,
then start the exact application image with `--image-volume=ignore` (the base
inherits `/var/lib/mysql` VOLUME metadata), all capabilities dropped and
`no-new-privileges`. No app privilege or database admin credential is required.

Setting `PODMESH_HOST_ADAPTER_SOCKET` requires an explicit MariaDB profile and
non-system application UID at daemon startup. An unreachable provider never causes
local Podman fallback. Host machine identity, boot ID, btime and synchronization status (one validated
observation per boot pass), capacity, storage assessment and
owned-container cgroup observations come from the provider's host namespace.
The node retains its private operation journal; intent/attempt commits before
sending an effect, and observed result commits afterward.

## Current executable scope

The transport carries the eight lifecycle operations on isolated flat universes and
nested universes granted explicitly by host policy, including stopped mount-free
cloning and scoped snapshot cleanup. Nested clone targets require their own UUID
grant for the original base image. Source profile and privilege are preserved.
The provider enforces its own observed container ID records, finite identity budget,
image policy, memory/CPU ceilings and network-none. Flat creates drop capabilities
and set no-new-privileges. Nested grants explicitly authorize the Rule11 outer
privileged device envelope; they do not inherit a grant from the application.

`allowed_nested` defaults to an empty array. Each explicit grant has this shape:

```json
{
  "universe_uuid": "<exact-existing-operation-universe-UUID>",
  "image": "sha256:<64-hex-already-local-base-image>",
  "mounts": [],
  "namespaces": "private-pid-ipc-uts-network-none",
  "devices": "privileged-host-device-access"
}
```

The exact empty host-mount envelope is enforced. Fixed provider create uses
`--image-volume=ignore` so OCI VOLUME declarations cannot create implicit host
volumes, and verifies observed mounts, privilege and namespace modes. No application-supplied mount,
namespace or device flags cross IPC. The application account remains unprivileged;
only the host provider creates a granted outer universe with this envelope.
This primitive alone proves neither inner Podman startup, workload continuity nor
nested migration. Named engineering fixtures exist separately for flat and nested
(`PODMESH_HOST_ADAPTER_TEST_IMAGE` and `_NESTED_IMAGE`); only explicit local images
arm host effects. These tests need a separately authorized host fixture.

Managed-network lifecycle, secret operations, manager host-state mount claims,
scoped universe statistics, migration/replication effects and the imported lease/
epoch boot policy remain unported to the private node/provider path. Refusals do
not close their required product coverage. Volume declare/grow remain app-journal
capacity declarations/assessment, not proof of physical host storage growth.

App-store ordering/replay and provider recovery must be qualified together: inspect
and retry the original operation after provider/application restart; refuse replaced
containers, wrong peers, wrong operation bindings and out-of-policy images/resources.
Preserve policy, provider identity records, manifest/image lock, private store dump
and application/DB volumes in restoration. Prove eight real lifecycle effects and
nominal boot return with exact artifacts; parser tests cannot close the unit.
