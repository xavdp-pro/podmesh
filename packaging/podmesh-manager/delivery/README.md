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
controller, native recovery helper, public manifest, README, LICENSE and NOTICE. No source compilation,
pull, tag, account, service, maintscript, credential, private configuration, live
store or automatic activation is part of package construction/install. Node
packaging is a separate immutable delivery recipe and remains unchanged.

## Operator inputs and new instance

Use an absent lowercase scope of1–15ASCII letters/digits/hyphens beginning with a
letter. The controller refuses another non-rolled-back instance, existing scope
paths/units/resources, conflicting numeric UID/GID1103 and an active/enabled legacy
`podmesh-manager.service`. Other externally named manager services are a runtime-owner
preflight responsibility; this is a new isolated scope, never an implicit cutover.
No host account is created or reassigned and no existing service is stopped.
The runroot is the root-owned0700 child `r` inside the same protected instance
root. Its computed byte length must be at most50 before any bundle read/provider
effect. With the fixed instance base,15-character scopes exactly fit.
Scopes such as `example-source` and `example-restore` fit this bound;
longer scopes refuse before creating state.
The runroot remains in receipt-bound storage.conf and full capture inventory;
there is no external/shared path or hidden path shim. Verify path constraints
against the installed engine before creating either declared unit.
Host prerequisites are Python3.11 or later, Podman, systemd, `iproute2`
(`/usr/sbin/ip` and `/usr/bin/ss`), GNU tar, libc6>=2.34 and libgcc-s1. Package dependencies
declare them; no tool installation occurs during controller execution.

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
shutdown, private SQL/AUTH journals and retained data. Binary/image journal tests
do not establish the installer's lifecycle. This README defines product commands;
runtime execution and activation require an operator's deployment mandate.

The actual startup/failed-start scenario must exercise the `Type=simple`
launcher/`ExecStartPost` boundary: a launcher PID alone is not container readiness.
Observe the real container Running/PID/socket/SQL transitions; distinguish
Podman exec125 (container/transport failure) from SQL client1 (connection/access
not ready). Neither is a successful readiness result. Preserve any failure trace
and correct an actual startup defect before candidate acceptance; no fake ready
fixture or automatic signal fallback substitutes for this VM proof.

## Native same-host recovery candidate

Native recovery uses an immutable recipe/package and the exact binary,
application OCI and private DB OCI pins declared above.
Nine package payload files include `recovery.py`;
there are no maintscripts, auto-start, accounts or embedded private inputs.

Only a newly created instance of this same native package can be a source.
The source and target use the same actual machine-id, original replica/host UUIDs,
logical topology, HMAC pair keys and configuration grants. Target scope, resource
IDs, bridge, units and receipt are new. Source endpoint is released then reused;
nonlocal peers keep their exact original configuration. No recursive recovery,
different-host migration, new daemon, raw libpod editing or receipt adoption exists.

Explicit root commands under the operator's deployment mandate:

```sh
python3 "$BUNDLE/instance.py" --scope "$SOURCE_SCOPE" capture --capture-id "$CAPTURE_UUID"
# Runtime exports/verifies capture.json, source-full.tar, store.sql and every OCI
# off guest, then supplies an independently verified root0600 checkpoint.
python3 "$BUNDLE/instance.py" --scope "$SOURCE_SCOPE" release-for-restore \
  --capture "$CAPTURE_DIRECTORY" --transfer "$TRANSFER_CHECKPOINT"
python3 "$BUNDLE/instance.py" --scope "$NEW_TARGET_SCOPE" restore \
  --capture "$CAPTURE_DIRECTORY" --network-plan "$NEW_TARGET_NETWORK_PLAN" \
  --recovery-id "$RECOVERY_UUID"
python3 "$BUNDLE/instance.py" --scope "$NEW_TARGET_SCOPE" start
python3 "$BUNDLE/instance.py" --scope "$NEW_TARGET_SCOPE" verify-restored \
  --operation-id "$FRESH_OPERATION_UUID"
```

Capture requires a started source with its DB available. It first refuses extra configuration, API files, foreign graphroot resources
or unknown VFS layers. Actual typed APP shutdown is required; the DB remains live
for a complete application-account dump and all five table/trigger snapshots.
Zero unresolved outbound requests/inbound replies is mandatory. After clean stop,
the capture preserves the complete instance tree, metadata (UID/GID/mode/mtime,
xattrs/ACLs, sparse extents, hardlink groups, symlinks), root configuration,
both physical volumes, images and observed units/resources. Access times are not
part of equality. An original observation owned by the original grant holder is
required for preserved operation replay. No fabricated qualification scope/grant.
An intent and `capture-in-progress` phase are saved before shutdown. The APP unit
condition refuses both that phase and `capture-stopped`, including direct unit
start. An interrupted capture preserves its intent and requires diagnosis; it is
never reset to Started or automatically resumed.

The bounded VFS inventory is explicit: storage JSON resource IDs, complete parent
layer chains, exact payload layer directories, exact two `_data` volume trees and
named engine metadata/lock files. Unknown layout/content refuses before
source release. APP/infra writable diffs must be empty; DB diffs are limited to
`/run`, `/tmp`, `/var/run`, `/var/tmp`. Extra durable configuration or a durable
writable layer cannot be justified by retaining its raw archive. Installed Podman
must prove `network_config_dir` and `network create --interface-name` support;
each scope has a hash-derived private bridge and only its own network metadata.
Qualification must observe the actual supported layout; a synthetic fixture is insufficient.
The validator checks the actual closed schema and identities, rather than accepting
or rejecting by version number alone. Engine version is recorded as provenance;
release/restore/verify require it unchanged. A stopped fixture on one engine version
does not qualify nominal execution on another. The deployment must present its
actual state to the strict validator and prove full SQL/functional restoration. Any different or unknown
metadata path still refuses; there is no implicit version upgrade or schema fallback.

Off-guest verification checkpoint has exactly these fields:
`verified_offguest:true`, `manifest_sha256`, `archive_sha256`, `sql_sha256`.
It is an explicit root operator attestation; helper does not contact off-guest
storage. Release saves an intent before its own controller rollback, verifies units,
non-volume resources, bridge and endpoint gone, and records live rolled-back receipt
hash alongside the unchanged captured receipt. Retained source data is never erased
and source APP must never resume. A partial release preserves the intent/state and
requires diagnosis; no automatic source restart or forged completion.

Restore reconstructs the complete immutable input in a distinct protected staging
tree, using named-member archive validation and manifest-defined creation. Hardlink
groups accept GNU tar's first occurrence independently of lexical manifest anchors.
No archive-controlled path extraction. Every configuration input, image, volume,
container, infra, network and unit has explicit old/new identity or equality mapping.
Supported engine paths/locks/PIDs are regenerated; raw source graphroot remains
immutable evidence and is not the target's live libpod database.

APP volume and both private configuration trees are restored with their complete
metadata. DB physical bytes are retained in immutable
input; the declared exception uses a NEW final DB volume, proves application user
1103 access with zero tables/triggers, imports the exact dump and compares all five
tables, schema3, eight immutable triggers/DEFINER/grants and deterministic full dump
before APP can start. It never empties an existing copied DB volume. Partial import
keeps `sql-import-intent`; both controller and APP ExecCondition refuse activation.
No alternate SQL-only/images-only restoration fallback exists.

Functional verification proves original operation request/result/receipt replay,
then persists one absent operation's intent before a real UID1103 append in the
original owned scope. It binds the result/receipt and actual new fact/hash to SQL,
preserves every old row in all five tables, retains identity/schema and triggers,
requires `every_peer` catchup and fresh linked authenticated receipt acknowledgements
from both original peers for the complete new history. PID changes refuse retained
proof continuation. Runtime must independently query both remote SQL stores; local
acknowledgements alone do not qualify the fleet. No HA, Maker/Logger or selected
release qualification is inferred from this helper.

## Qualification gates

Run `test_delivery.py` and `test_recovery.py` on the explicitly designated build
host. Recovery suite includes one deliberately allowed bounded GNU tar process for
the sparse/xattr/hardlink roundtrip; reject all other real subprocesses. It covers
configuration/graphroot/layer closure, durable-layer refusal, scope ownership,
all-history preservation, strict fresh two-peer catchup, receipt links and unsafe
archive members. Recorder suite additionally covers partial-import APP start refusal.

Before package acceptance, run a separate real MariaDB roundtrip using the
exact pinned DB image and native helper `oracle`, `sql_dump`, `immutable_rows`,
`trigger_rows` through actual UID1103. Provision two isolated private DB servers
with explicit app credentials, exact controller hostnames and no ambient DSN or
shared state. Confirm both initially have zero application tables/triggers. Seed
SOURCE through the pinned product binary with schema3, eight triggers and all
five tables populated (including genuine observation and accepted AUTH journal).
Persist source SQL rows/identity/grants/trigger metadata, stop its APP with typed
ack, require zero pending exchanges, and export the helper's exact deterministic dump.
Prove TARGET empty with `oracle(empty=True)`, import exact dump bytes as app UID1103,
then require dump hash/full rows/trigger/DEFINER/grant equality. Attempt UPDATE and
DELETE of a known immutable fact and require actual trigger refusal with unchanged
rows. Passwords stay in protected mounted files, never argv/log/public evidence.
Use separately owned fresh namespaces and cleanup evidence. Fixture SQL roundtrip
is an engineering gate; it does not replace the same-package VM functional journey.

Review committed source before dispatch. Build two identical packages from the
pinned source, audit nine payloads/manifest/root ownership/control-only/dependencies
and preserves previous artifacts. Runtime then qualifies that exact package's fresh
install/start/stop/rollback-repeat, full closed capture/release/empty-target restore,
actual PID/UID/binary/image identity and fresh original-peer SQL convergence.
Acceptance requires independently verified evidence for every stage.
