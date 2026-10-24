# Canonical MANAGER: application and private MariaDB in one container

Source candidate for a **new** manager replica instance. This is a separate image
recipe, not a conversion of the existing two-container private delivery package.
The existing `private-unit` and `delivery` recipes remain unchanged. No existing
manager, credential, profile, database or volume is adopted, reset or removed.

The root PID1 starts one private MariaDB with `--skip-networking`, prepares the
**manager** schema using `podmesh-store-admin prepare --role manager`, then runs
`podmesh-managerd` as `podmesh-manager`, UID/GID1103, without supplementary groups
or capabilities. Application system user = DB user = database = `podmesh-manager`.
Only table-scoped SELECT/INSERT are granted; `store_schema` is SELECT-only.
The application cannot perform DDL or administer its private server.

The application connects only to `/run/podmesh-manager/mysql/mysql.sock` using its
own credential. Separate root administration credentials never enter its process
environment. MariaDB has no TCP listener. The application's authenticated static
peer listener is retained: this unit is **not** a socket-only manager network.
It acquires no activation/vote authority. No NODE, host capability adapter, engine
socket, host root mount or privileged mode belongs in this container.

## Existing contracts used

- `experiments/manager-resident/src/cli.rs`: exact config/state/runtime flags,
  pure `--validate-config`, explicit authenticated-static-peers mode, private
  runtime directory, database path directly below the application state directory.
- `src/lib.rs`: explicit profile environment takes precedence over the legacy
  `.sqlite` anchor; MariaDB opens verify the current manager schema without DDL.
- `src/bin/podmesh-store-admin.rs`: separately authorized root schema preparation.
- `experiments/manager-resident/src/lib.rs`: typed `status` and `shutdown`, with
  EOF-framed Unix control requests and a graceful worker join on shutdown.
- Existing `packaging/podmesh-node/canonical-unit/entrypoint.py`: copied unchanged
  into the image as `lifecycle.py`, with its own hash. The manager subclass reuses
  process creation, bounded output draining, owned child reaping, SQL helpers and
  application-before-database stop ordering. The fixed module path bindings select
  MANAGER's directories. No NODE code or source file is modified or executed as a
  NODE. Manager identity/configuration, birth and control adaptation are explicit.

## Durable layout and configuration

Declare separate root-owned0755 writable mounts at `/apps/podmesh-manager/etc`
and `/apps/podmesh-manager/sav`, both on durable storage. Runtime `/run` must be
fresh for each container creation. Recreate the container to restart, preserving
exact etc/sav mounts and the image; an in-place restart with stale `/run` refuses.

Before first start, supply `etc/config.json`, root:1103 mode0440, with the real
manager configuration. Keep its UUIDs, full topology, scopes, pair keys, timing and
peer endpoints; the entrypoint does not generate substitute identities. Set:

- `network.database_path=/apps/podmesh-manager/sav/app/manager.sqlite`: only a
  resolution/lock anchor; it must not become a SQLite database;
- `control_socket=/run/podmesh-manager/api/control.sock`;
- `observation_writer_uid=1103`, `votes` absent/null;
- `network.bind` an explicit IPv4 address and unprivileged peer port other than3306.

The actual daemon validates the full configuration as UID1103 before DB birth.
The wrapper keeps a SHA256 of the complete private config in the birth receipt;
clean recreation requires the same config, credentials, profile, provenance and
DB directory identity. Peer changes/upgrades and unclean recovery need their own
procedure; this initial recipe does not silently repair state.

PID1 creates separate private credentials under `etc/mysql/localhost/passwd` and
`etc/admin/passwd`, plus root:1103 mode0440 `etc/store.json`. Database data lives
in `sav/mariadb`, application locks/state in `sav/app`, and private birth receipts
and bounded diagnostics in `sav/admin`. No private input or secret is embedded in
an image or returned to standard output.

Local `status` availability establishes that this replica's current store is open
and its identity is correct. The receipt records `catch_up_at_start` separately;
a running replica is not a claim of convergence, quorum or active-manager authority.
On SIGTERM, PID1 sends typed `shutdown`, allows the existing manager delivery's
30-second graceful window for exit0/socket removal, then stops
MariaDB. Forced cleanup is recorded as failed, not as a clean restart or backup.

## Build on the declared build CT only

Use a clean committed source checkout at `$SOURCE_PIN` as the build context, with
exact built executables named `podmesh-managerd` and `podmesh-store-admin` at its
root. The manager executable is Cargo's `podmesh-manager-resident-lab`, installed
under its package-facing name and built with feature `mariadb`. Supply its real
binary source pin and hash separately from the recipe pin. The current legacy
private delivery manifest is not the manifest of this new canonical image.

Reuse the existing verified `python-closure/` input and unchanged
`packaging/podmesh-node/canonical-unit/install-python-closure.sh`. There is no
new Python acquisition or dependency installer. The pinned local base must already
exist. Do not build on the operator workstation.

```sh
podman build --network=none --platform linux/amd64 --pull=never \
  --file packaging/podmesh-manager/canonical-unit/Containerfile \
  --build-arg MANAGER_BINARY_SHA256="$MANAGER_BINARY_SHA256" \
  --build-arg ADMIN_BINARY_SHA256="$ADMIN_BINARY_SHA256" \
  --build-arg SUPERVISOR_SHA256="$SUPERVISOR_SHA256" \
  --build-arg GRANTS_SHA256="$GRANTS_SHA256" \
  --build-arg LIFECYCLE_SHA256="$LIFECYCLE_SHA256" \
  --build-arg BINARY_SOURCE_REVISION="$BINARY_SOURCE_REVISION" \
  --build-arg ADMIN_SOURCE_REVISION="$ADMIN_SOURCE_REVISION" \
  --build-arg RECIPE_REVISION="$SOURCE_PIN" \
  --build-arg PYTHON_CLOSURE_SHA256="$PYTHON_CLOSURE_SHA256" \
  --build-arg PYTHON_INSTALLER_SHA256="$PYTHON_INSTALLER_SHA256" \
  --tag "$NEW_APPLICATION_IMAGE" .
```

Hashes correspond to actual context bytes: SUPERVISOR is this entrypoint;
LIFECYCLE is the unchanged NODE source copied as `lifecycle.py`; GRANTS is this
role's `grants.json`. Record/export the resulting exact image ID/manifest/archive
hash, binary pins and recipe pin before a separately authorized native run.

Affected CT source tests, unexecuted in this source lot:

```sh
python3 -B packaging/podmesh-manager/canonical-unit/test_entrypoint.py
```

This suite exercises actual local IPC/owned children for acknowledged and rejected
shutdown after PID1 received its stop signal. It does not mock MariaDB or qualify
it. Existing NODE109 proof is retained, with no NODE rerun requested. No Rust code
changes or new Rust test suite are needed for this packaging-only candidate.

## Native handoff command (requires its own runtime authorization)

Inputs `$NEW_ETC` and `$NEW_SAV` must be new, separate, absolute directories on
durable storage, root-owned0755. `$PRIVATE_MANAGER_CONFIG` is a private existing
configuration using the paths above and the intended real replica identities.
Use an existing explicitly permitted private network and an unused container name;
never publish3306 or use host networking. The image's default PID1 remains root;
its application drops to1103. Use standard native Podman capabilities, as in the
existing lifecycle, without an extra privilege/resource wrapper.

```sh
set -eu
test ! -e "$NEW_ETC" && test ! -e "$NEW_SAV"
install -d -m0755 "$NEW_ETC" "$NEW_SAV"
install -m0440 -o0 -g1103 "$PRIVATE_MANAGER_CONFIG" "$NEW_ETC/config.json"
podman run --detach --name "$NEW_CONTAINER" --pull=never --image-volume=ignore \
  --network "$PRIVATE_MANAGER_NETWORK" --publish "$PEER_BIND:$PEER_PORT:$PEER_PORT/tcp" \
  --stop-timeout=60 --tmpfs /run:rw,nosuid,nodev,mode=755 \
  --mount "type=bind,source=$NEW_ETC,destination=/apps/podmesh-manager/etc" \
  --mount "type=bind,source=$NEW_SAV,destination=/apps/podmesh-manager/sav" \
  "$EXACT_MANAGER_IMAGE_ID"
```

Run the native check on the authorized guest, not P3: observe both actual children
and their UIDs, MariaDB's Unix socket/no TCP listener, control `status` and
`catch_up`, then one typed `append_observation` using a configured locally owned
scope. Requests must half-close their write side before reading the JSON reply.
For a durable read, use the actual daemon as the application identity:

```sh
podman exec --user 1103:1103 \
  --env PODMESH_STORE_PROFILE=/apps/podmesh-manager/etc/store.json \
  "$NEW_CONTAINER" /usr/lib/podmesh-manager-canonical/podmesh-managerd \
  --inspect-store --config /apps/podmesh-manager/etc/config.json \
  --state-dir /apps/podmesh-manager/sav/app
podman stop --time 60 "$NEW_CONTAINER"
```

Require container exit0 and birth phase `stopped-clean`, then
remove only that exact stopped container ID. Recreate with the same run command,
new unused container name, same image and etc/sav, then read the prior observation
and verify unchanged manager identity/config/profile/credential hashes and DB
inode. Stop the second container cleanly and leave persistent state for inspection.
This candidate has no native result yet; restore, replication and production
activation are separate milestones.
