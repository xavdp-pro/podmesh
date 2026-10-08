# Immutable private manager delivery

Source candidate recipe, not a qualified installer or production activation.
This separate package reuses manager binary source
`6ba91890a8ff24230064ce3c709f563c85f4370e` and its already-built private application
image. It does not rebuild either, reuse the host-service `.deb`, relaunch a completed
campaign or acquire Governor/Maker authority. Installation/start/stop/rollback of
this **same** new `.deb` needs separate qualification after source review.
The [unit contract](../../../docs/FUNCTIONAL-UNITS.md) remains authoritative.

## Immutable inputs

On the declared build host, using a clean committed delivery source archive:

```sh
python3 packaging/podmesh-manager/delivery/build-deb.py \
  --kit "$MANAGER_PAYLOAD_DIRECTORY" --manifest "$MANAGER_BUILD_MANIFEST" \
  --database-oci "$DATABASE_OCI_ARCHIVE" \
  --recipe-revision "$FULL_RECIPE_REVISION" --output "$NEW_OUTPUT_DIRECTORY"
```

The payload directory contains `podmesh-managerd` and `application.oci.tar` directly.
Required binary SHA256 `71bc510ac369a271141ec25d98867563c0da77324a358902e3136fb2c324ad29`;
app OCI archive `17da3325c3a2a72d6f7fdf1485a5a1c37803a979c670938abdf6b4629fe900d7`;
app config image `sha256:3098a589d3a7c946d1ded0777b5ab273cdfb8320c46f7c9e65af1a2a43af6bfc`;
app OCI manifest `sha256:73ff161f7183acb8f5da4a2f4fd0d89a8b5d2b6f22d74a5ad5c76226f2f91c43`.
The DB archive is the independently exported immutable base:
SHA256 `087a378584383840073ebc347e383229f9765572785b7b4cbf09f22a274b4efb`,
config `sha256:53ef799caed285438d88678b529b4ff24406d4788f47ab8a74bb2212c707899a`,
export manifest `sha256:4ca2b8d82f602cefca23a2f270a591c6b27e27d81d5e32ca8ecdf92cdad50a15`.
The export manifest is distinct from upstream registry manifest `93fc3fe...`.

Before creating output, the builder verifies exact archives, source manifest,
config/platform/entrypoint/account, descriptor sizes/hashes and each uncompressed
rootfs diff ID. It reads tar members without extracting paths. Both archives and
the exact executable are copied into a versioned payload-only package with the
controller, public manifest, README, LICENSE and NOTICE. No source compilation,
pull, tag, account, service, maintscript, credential, private configuration, live
store or automatic activation is part of package construction/install. Node
packaging is a separate immutable delivery recipe and remains unchanged.

## Operator inputs and new instance

Use an absent lowercase scope of1–32ASCII letters/digits/hyphens beginning with a
letter. The controller refuses another non-rolled-back instance, existing scope
paths/units/resources, conflicting numeric UID/GID1103 and an active/enabled legacy
`podmesh-manager.service`. Other externally named manager services are a runtime-owner
preflight responsibility; this is a new isolated scope, never an implicit cutover.
No host account is created or reassigned and no existing service is stopped.

Provide existing regular root-owned0600 files:

- Manager configuration with canonical logical/replica/host UUID topology, exact
  nonlocal peer list and distinct32byte hex HMAC pair keys. Local host UUID must
  equal this host's `/etc/machine-id`. `observation_writer_uid=1103`,
  `control_socket=/run/podmesh-manager/control.sock`,
  `network.database_path=/var/lib/podmesh-manager/manager.sqlite` (profile resolution
  anchor only; explicit MariaDB prevents SQLite fallback),
  `network.bind=0.0.0.0:<declared-pod-peer-port>`; no vote configuration.
- Network plan with the exact fields below. This surface currently declares IPv4
  only. Port must be1024–65535 and distinct from3306. Publication, if any, uses a
  specific IP actually owned by the host, never wildcard, same peer port both sides.
- Two distinct single-line DB password files: application and administrator.
  Neither password enters the package, process argv or a public manifest.

```json
{
  "subnet": "<explicit-new-/24-through-/28-private-bridge-CIDR>",
  "gateway": "<declared-gateway-in-subnet>",
  "pod_ip": "<distinct-declared-app/DB-pod-address-in-subnet>",
  "peer_container_port": 19543,
  "peer_publish": {"host_ip": "<specific-owned-host-IP>", "host_port": 19543}
}
```

`peer_publish:null` publishes nothing. In that case peer routing to the private pod
addresses is an explicit deployment prerequisite, never assumed. A bridge, route,
firewall and authentic pair configuration must be part of the future declared plan;
this recipe never changes host firewall or grants peer authority. Existing live
routes and configured network subnets must not overlap the new bridge.

```sh
sudo /usr/lib/podmesh-manager-private/"$BUNDLE"/instance.py --scope "$SCOPE" prepare \
  --configuration "$PRIVATE_MANAGER_CONFIGURATION" --network-plan "$ROOT_NETWORK_PLAN" \
  --application-password "$APPLICATION_PASSWORD_FILE" \
  --database-root-password "$DATABASE_ROOT_PASSWORD_FILE"
sudo /usr/lib/podmesh-manager-private/"$BUNDLE"/instance.py --scope "$SCOPE" start
sudo /usr/lib/podmesh-manager-private/"$BUNDLE"/instance.py --scope "$SCOPE" status
sudo /usr/lib/podmesh-manager-private/"$BUNDLE"/instance.py --scope "$SCOPE" stop
sudo /usr/lib/podmesh-manager-private/"$BUNDLE"/instance.py --scope "$SCOPE" start
sudo /usr/lib/podmesh-manager-private/"$BUNDLE"/instance.py --scope "$SCOPE" rollback
```

## Boundary, readiness and shutdown

Root administration owns new protected configuration/receipt paths and an exclusive
VFS graphroot/runroot/tmp. Both exact images load there; no host default store,
shared server or previous journal is adopted. Resources have scope/bundle labels
and durable creation intents before effects, followed by observed IDs. Interrupted
creation without an observed ID requires review; name reuse does not adopt it.
Network ID, recorded resource IDs, configuration hashes, machine identity and unit
hashes gate subsequent commands.

The private app/DB pod shares only network. App and DB have private PID/IPC/UTS,
512MiB memory/swap and1CPU ceilings each. DB binds `127.0.0.1:3306` only in that
namespace, its upstream service identity initializes a new separate volume.
Only the declared peer endpoint can be published by the infra container. Container,
pod/infra, exact peer publication and bridge/subnet identity are inspected. No
DB publication is permitted. Deployment must independently verify actual bridge
routing and the absence of unintended exposure. Before start, inspection checks
the declared private network/static-IP plan; it does not claim an assigned namespace.
Once infra runs, readiness additionally requires the exact actual network, IP and
gateway. A created pod is not a live network observation.

The application is actual numeric1103:1103, rootfs read-only without automatic tmpfs,
capabilities dropped, no-new-privileges and host user namespace. It gets exactly
own app-state volume, private runtime and read-only configuration/profile/password.
DB gets only its volume, admin secret and the application password file, never the
peer-HMAC configuration. All inherited image volumes are ignored. No Podman socket,
host root/proc/sys, host device grant or privileged mode enters the application.

The generated `.target` and DB/app services are installed only by explicit prepare,
never enabled at boot. Root Podman launcher identity is distinct from1103 application
identity. DB readiness first exercises actual application-account SQL over explicit
private TCP, with a file-read credential only in the child environment. App readiness
then checks socket0600 owner, kernel UID/GID1103, actual container PID and executable
hash. Status is read by `socat` inside the own container under1103 and checked for
same replica, open store, no activation authority and `catch_up.caught_up`.
A locally ready process is not authenticated all-peer convergence: `caught_up_by`
is recorded, and actual convergence still needs separate proof.

Stop requests the actual typed `shutdown` as1103, requires exact acknowledgement,
waits boundedly for own process PID0/runningfalse/exit0/noOOM and socket disappearance,
then stops explicitly all own services/target and checks DB exit0. No automatic
signal escalation substitutes for failed/timed-out application shutdown. The
attached root launcher disables signal proxy and the application service sends no
SIGKILL. An already stopped process is recorded honestly; a previous acknowledgement
is retained only for the same actual finish timestamp. Start failure uses these
same stop gates; failure preserves state and must not claim successful cleanup.
Do not mutate package units to weaken shutdown/refusal gates.

## Rollback and proof

Rollback validates unchanged units, original IDs/labels/network ID and absence of
foreign containers before stop; repeats the absence check after API shutdown before
removal. Only own stopped app/DB containers, pod/infra and bridge plus unchanged own
units are removed. Own app/DB volumes/data, image store, private HMAC/password/config,
identity receipt and readiness/shutdown evidence remain. No shared image prune,
recursive deletion, account change or legacy service administration. Repeated
completed rollback is inert. Recovery/upgrade against retained identities is a
separate path; rollback is not journal/history erasure or a fresh-ID substitution.

Before activation, declared build host runs syntax and focused recorder regressions,
two independent package assemblies from the same pinned inputs and full control/
payload/ownership/license/NOTICE/no-secret audit. Runtime must then qualify the same
verified `.deb` for install/start/stop/restart/refusal/rollback and independently check
actual UID, image/process hashes, store, mounts, cgroups, bridge/publication, typed
shutdown, private SQL/AUTH journals and retained data. The completed earlier manager
campaign proves its immutable binary/image journal behavior, not this new installer's
lifecycle. No runtime action, vote, production activation or previous campaign replay
is authorized by this README alone.
