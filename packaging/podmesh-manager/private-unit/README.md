# Private manager unit application wrapper

This is a bounded candidate recipe, not an installer or a SHAPER integration
claim. It reuses the exact CT-built `podmesh-managerd` binary, without compilation
or downloads in the recipe. The base and the separate database container both use
`docker.io/library/mariadb@sha256:93fc3fe333b6cdfb061425869c2c3a5bb2851c0c74dbebc2c940f024e7482d76`.
This is the confirmed Linux/amd64 registry manifest for MariaDB 11.8.9 on Ubuntu
24.04; its local config image ID is
`sha256:53ef799caed285438d88678b529b4ff24406d4788f47ab8a74bb2212c707899a`.
A config ID is not a registry pull digest. Verify both identities and platform on
the build host before construction. The inherited database entrypoint is replaced; the application container never
starts a database. A colliding UID/GID or account refuses the image build rather
than modifying an upstream identity.

## Build inputs and provenance

On the declared build host, create a private clean context containing this
`Containerfile` and only the exact manifest-selected executable named
`podmesh-managerd`. Require its manifest SHA-256 and the complete committed source
revision of that executable's source, independently from the recipe revision.
The pinned base already supplies `id`, `socat`, `mariadb` and `mariadb-dump`; the
recipe checks their execution as UID/GID 1103 and stores all resolved package
versions in `/usr/share/podmesh-manager/image-packages.tsv`. No Python or package
installation is required by this wrapper. For example, with supplied values:

```sh
podman build --platform linux/amd64 --pull=never --file "$CONTEXT/Containerfile" \
  --build-arg MANAGER_BINARY_SHA256="$MANAGER_BINARY_SHA256" \
  --build-arg BINARY_SOURCE_REVISION="$BINARY_SOURCE_REVISION" \
  --build-arg RECIPE_REVISION="$RECIPE_REVISION" \
  --tag "$APPLICATION_IMAGE" "$CONTEXT"
```

The recipe verifies the copied bytes before changing their mode. Record binary source
revision and recipe revision separately, binary hash, base manifest/config ID/platform, application image ID/digest, OCI export hash
and container user in the build manifest. Export that built image; runtime loads
and runs the same export and verifies image identity. A rebuilt sibling image is
not the qualified artifact. A hash label alone is not provenance: compare the
executable bytes extracted from the image with the CT binary manifest.

## Identities and mounts

The application account and primary group are `podmesh-manager`, UID/GID 1103,
with a non-login shell; container configuration defaults to that account. This
creates no host account and does not change an existing host UID 103. The private
DB application user and database are `podmesh-manager`, with only that database's
rights. Database bootstrap/admin credentials remain separate. The DB container
keeps its upstream service identity; do not override it to UID 1103.

Use separate owned persistent application-state and DB-data volumes. Configuration,
store profile, replica private kit and password file are read-only private mounts,
readable by numeric application UID 1103; directories 0700 and files 0600. Create
only new candidate paths/volumes, with explicitly verified numeric ownership; never
recursive-chown existing host service state. Control-socket authorization must name
this application's actual UID, and its API runtime directory must be owned by it.
Mount a writable private `/run/podmesh-manager` (tmpfs or dedicated runtime volume).
No Podman socket, host root filesystem, host database socket or privileged mode is
part of this application recipe.

## Private pod network and execution

Create a dedicated bridge network and pod for one replica/function; do not use
`--network host`. Publish only the peer endpoint (candidate default 19543) on an
explicit allowed host bind address. The two containers share the pod's private
network namespace; the profile connects to `127.0.0.1:3306` there. MariaDB has no
host port publication. Host firewall and peer allow-list remain explicit runtime
inputs. Pod isolation, private credentials and two owned volume sets form one
functional unit; two databases on a shared server do not.

An illustrative runtime sequence, after checking all names are unused:

```sh
podman network create "$UNIT_NETWORK"
podman pod create --name "$UNIT_POD" --network "$UNIT_NETWORK" \
  --publish "$PEER_BIND:19543:19543/tcp"
podman run --detach --name "$DB_CONTAINER" --pod "$UNIT_POD" \
  --mount "type=volume,source=$DB_VOLUME,destination=/var/lib/mysql" \
  --mount "type=bind,source=$DB_INIT_DIR,destination=/docker-entrypoint-initdb.d,ro=true" \
  --mount "type=bind,source=$DB_ADMIN_SECRET,destination=/run/secrets/db-admin,ro=true" \
  --env MARIADB_ROOT_PASSWORD_FILE=/run/secrets/db-admin \
  docker.io/library/mariadb@sha256:93fc3fe333b6cdfb061425869c2c3a5bb2851c0c74dbebc2c940f024e7482d76
# Check private DB readiness and scoped application credentials before starting app.
podman run --detach --name "$APP_CONTAINER" --pod "$UNIT_POD" \
  --mount "type=volume,source=$APP_VOLUME,destination=/var/lib/podmesh-manager" \
  --mount "type=bind,source=$PRIVATE_CONFIG_DIR,destination=/etc/podmesh-manager,ro=true" \
  --tmpfs /run/podmesh-manager:rw,mode=0700,uid=1103,gid=1103 \
  --cap-drop ALL --security-opt no-new-privileges \
  "$APPLICATION_IMAGE_ID"
```

All variables are required deployment inputs, not laboratory defaults. The DB init
material creates the scoped user/database and is private, separately authorized,
and never embedded in this image. Configure the replica's in-pod listener, peer
advertisement, private store/kit paths and control UID consistently. Mount any kit
paths outside `/etc/podmesh-manager` explicitly as read-only private inputs. Do not
alter existing G6 fixtures or silently reuse their topology/credentials.

Qualification must observe actual process UID/group, exact executable bytes,
network namespace and port publication, volume/private-file access, journal writes,
dump/empty-target functional restore and nominal replica output. Stop/start and
rollback preserve those owned volumes and identities. This wrapper neither qualifies
three-replica convergence nor grants vote keys, activation or host privilege.
