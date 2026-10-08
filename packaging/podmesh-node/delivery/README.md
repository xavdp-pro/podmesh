# Immutable private node delivery

This is a source recipe for a Debian **payload-only** package and explicit root
instance controller. It requires qualification of that same `.deb` for install,
start, stop and rollback on a declared isolated host. Source review and a successful
build do not prove runtime behavior, boot return, full host restoration or the
complete A/B target. Binary and image construction remain at source
`3199d784a6177fa7652588833091b35ab88ba5c9`; packaging revisions never recompile them.

## Build inputs and package boundary

On the declared build host, using a clean archive of the delivery recipe revision:

```sh
python3 packaging/podmesh-node/delivery/build-deb.py \
  --kit "$EXACT_NODE_PAYLOAD_DIRECTORY" --manifest "$EXACT_NODE_MANIFEST" \
  --database-oci "$EXACT_DATABASE_OCI_ARCHIVE" \
  --recipe-revision "$FULL_DELIVERY_RECIPE_REVISION" --output "$NEW_OUTPUT_DIRECTORY"
```

`--kit` has `podmeshd`, `podmesh-host-adapter`, `application.oci.tar` directly
under it. All three hashes, application manifest/image, source pin and actual
application configuration are checked before output creation. The database input
is separately fixed by archive hash
`087a378584383840073ebc347e383229f9765572785b7b4cbf09f22a274b4efb`,
export OCI manifest
`sha256:4ca2b8d82f602cefca23a2f270a591c6b27e27d81d5e32ca8ecdf92cdad50a15`
and config image
`sha256:53ef799caed285438d88678b529b4ff24406d4788f47ab8a74bb2212c707899a`.
That export manifest is distinct from upstream registry manifest `93fc3fe...`.
The builder reads archive blobs without extracting archive-controlled paths and
checks every descriptor size/digest. No pull, build, tag or runtime activation.

A package has a name and `/usr/lib/podmesh-node-private/<bundle>/` path containing
both source and recipe revision prefixes. It contains exact executables, both
OCI archives, the controller, this document and a public payload manifest. No
maintainer scripts, host account creation, service installation, credentials,
profiles, live data, policy grants or automatic service activation. Dependencies
are external host prerequisites. Archive provenance and licensing notices remain
in the image payloads; the delivery manifest records all payload and `.deb` hashes.
SOURCE_DATE_EPOCH fixes package source time; repeatability must be measured on the
build host, never assumed from that setting.

## Prepare a new declared instance

Install the reviewed package using the host's normal root administration path.
Installing it alone starts nothing. Copy the sample policy from
[private-unit](../private-unit/host-policy.example.json) into an external root0600
file. Declare a lowercase ASCII scope of1–32letters/digits/hyphens beginning with
a letter, UID/GID1102, exact allowed local image IDs, finite
resource ceilings and `state_dir=/var/lib/podmesh-node-private/<scope>/provider-state`.
Nested UUID/base-image/empty-mount grants require the separate explicit mandate
specified by [the provider contract](../../../docs/HOST-ADAPTER-CONTRACT.md).

Supply two **different**, existing regular root-owned0600 credential files,
one application DB credential and one DB administrator credential. Neither is a
package input. A policy is an operator capability grant, not authority issued by
PodMesh. The provider performs its complete policy validation before DB/app start.

```sh
sudo /usr/lib/podmesh-node-private/"$BUNDLE"/instance.py --scope "$SCOPE" prepare \
  --policy "$ROOT_POLICY" --application-password "$APPLICATION_PASSWORD_FILE" \
  --database-root-password "$DATABASE_ROOT_PASSWORD_FILE"
sudo /usr/lib/podmesh-node-private/"$BUNDLE"/instance.py --scope "$SCOPE" start
sudo /usr/lib/podmesh-node-private/"$BUNDLE"/instance.py --scope "$SCOPE" status
sudo /usr/lib/podmesh-node-private/"$BUNDLE"/instance.py --scope "$SCOPE" stop
sudo /usr/lib/podmesh-node-private/"$BUNDLE"/instance.py --scope "$SCOPE" start
```

All commands refer to that package's exact controller, never an unversioned shared
entrypoint. Preparation refuses existing instance paths, unit/resource names,
other non-rolled-back node instances, conflicting host UID/GID1102 accounts,
and an active/enabled legacy `podmesh.service`. It creates no host account and
never stops, disables or modifies the legacy service. Another legacy name or
external node is a runtime-owner preflight responsibility; this recipe is for a
new isolated host scope, not a discovery-based production cutover.

Preparation creates root-protected private paths, a new exclusive VFS graphroot,
runroot and engine tmp directory. Both OCI archives load into **that** store, never
the host's default Podman store. New resources receive instance/bundle labels;
intent records precede creation and observed IDs precede subsequent administration.
An interrupted create without an observed ID is preserved for explicit review,
never adopted by name. Configuration hashes and original machine identity gate
later commands. There is no in-place upgrade, automatic identity adoption or
credential rotation surface in this slice.

## Explicit topology and supervision

| Resource | Identity and scope |
| --- | --- |
| Application container | Exact image `sha256:505fdb59...`, actual UID/GID1102, read-only rootfs with no automatic tmpfs, all capabilities dropped, no-new-privileges, host user namespace, private PID/IPC/UTS |
| Private DB container | Exact image `sha256:53ef799c...`, upstream server identity and entrypoint, own database/user `podmesh-node`, own bootstrap admin credential |
| Private pod | Network-none; app and DB share loopback only; DB binds `127.0.0.1:3306`; no published host ports |
| App volume | New explicit owned persistent volume, numerical1102 ownership; never chown existing state |
| DB volume | Separate new explicit volume, upstream initialization; never manager/shared DB state |
| Host provider | Exact separate executable, root service with root0600 policy and root0700 state; only typed peer-checked socket crosses to app |
| API directory | New1102:1102 mode0700 directory, API socket0600; host administration traverses root-protected parent |
| Provider socket directory | Root:1102 mode0750, endpoint0660, mounted read-only in app; policy/state never mounted there |

The DB gets its own admin/password bootstrap mounts. The application gets only
its profile/password, application data, API directory and provider socket
directory. Neither gets host root, host `/proc` or `/sys`, policy/provider state or
a full Podman socket. Both use `--image-volume=ignore`: inherited image VOLUME
metadata must not create anonymous storage. Container inspection checks exact
mount destinations/sources/access, volumes, IDs, images, pod/infra network-none, privileges and
published ports before service readiness. App and DB have declared512MiB memory
and swap ceilings and1CPU each. These limits require a matching host capacity
preflight; workload ceilings are separately granted in provider policy.

Per-instance `.target`/provider/DB/app systemd units are generated without enabling
boot activation. Provider readiness precedes private DB application-account access,
which precedes app readiness. Root Podman launchers are distinct from the actual
1102 application process. Readiness verifies kernel socket peers, running executable
hashes and actual container PID. DB access uses numeric1102 `podman exec`, explicit
TCP parameters and a file-read credential in the child environment; no credential
in argv or copied child stderr. No new readiness sidecar is created.

Start failure stops only the own target and retains private state. Stop removes no
workload, volume, image or journal. Socket cleanup only removes a recorded socket
inode/owner after its recorded service has stopped; a replaced or unrecorded
endpoint refuses and remains for review. Automatic daemon restart is deliberately
not enabled: an abrupt termination before recording readiness requires ownership
review. Actual host boot/API boot-pass activation needs a separate explicit mandate
and qualification; this recipe never enables a unit, requests reboot or fabricates
an API boot-return result. Stop records actual container PID/running/exit/OOM/finish
observations and refuses success after a force-kill. `podmeshd` has no typed
clean-shutdown API in this pin: SIGTERM/process exit is not proof of a drained
journal. The runtime owner must inspect pending attempts and prove restart/replay;
this recipe does not claim that stop completed every in-flight operation.

## Bounded rollback of the same package

First perform workload stop/delete through the **application API** and preserve
its SQL journal. Stop alone leaves workload effects intact. Then:

```sh
sudo /usr/lib/podmesh-node-private/"$BUNDLE"/instance.py --scope "$SCOPE" rollback
```

Before stopping anything, rollback refuses any remaining universe/foreign container
in this exclusive store, including a recorded container whose labels changed.
It checks unit hashes and each resource's recorded ID and labels. It stops/disables
only own units, repeats the workload-absence check after API shutdown, removes only
verified stopped app/DB containers and the recorded
pod/infra, removes only unchanged own unit files, and reloads systemd. Existing
host accounts/services and default Podman resources are untouched. Uncertain
ownership, missing IDs and externally changed units refuse instead of deleting.
Repeating a completed rollback is inert.

**Retained:** app and DB volumes/data, private images/graphroot, provider identity
records, root configuration/credentials, receipt and qualification evidence. A
rolled-back receipt is not reused; restarting or upgrading against those identities
requires a separately defined migration/recovery path. The package can be removed
after rollback without maintscripts deleting retained state. Rollback is removal of
its own control plane, not reversal of historical application operations or a
promise to rebuild host effects from SQL alone.

## Required verification before candidate activation

On the build host, review/import the immutable recipe, run Python syntax and the
focused recorder tests (`python3 packaging/podmesh-node/delivery/test_delivery.py`),
construct the package twice from the same pinned inputs,
compare `.deb`/manifests and audit `dpkg-deb --contents`/control for absence of
maintainer scripts and secrets. Do not rebuild the node executables or OCI images.
On the authorized isolated runtime host, use that same verified `.deb` for prepare,
start, stop/start and rollback; independently inspect actual UID, peers, image and
binary hashes, private server identity, mounts, ports, cgroups and SQL durable
lifecycle/replay via [the proof driver](../qualification/README.md).

Exercise conflicting identity/service/path refusal, changed labels/ID/unit refusal,
retained-workload rollback refusal, interrupted preparation preservation and exact
owned cleanup. Record actual start failure and socket-restart behavior. Full node
host-storage restoration, physical growth, nested inner workload continuity and
unported A/B capabilities remain separate required product work and proofs.
