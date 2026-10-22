# Canonical NODE unit — unbuilt, unqualified source candidate

This **new** recipe is a single container: root PID1 supervises MariaDB as mysql
and the exact NODE binary as Linux UID/GID1102, system user/database user/database
`podmesh-node`. It replaces no installed unit and does not alter the old
`private-unit`/`delivery` recipes or their scoped historical PASS. PodMesh is not
a five-service SHAPER core universe. Product adoption remains SHAPER OS
`173ae591988e1f71a33f65289fc26b9de4aaf3d7`; the current operator's Rules4/26 and
Turbinobash direction define the target, not a new conformity claim.

## Required immutable inputs and dependency gates

Build context: this Containerfile, entrypoint.py, grants.json, plus the exact
`podmeshd` and `podmesh-store-admin` binaries. Supply mandatory SHA256 values for
all four payloads, full binary/admin source pins and recipe pin. The build writes
provenance, and startup rechecks all payload bytes. Build on the declared build host,
with `--network=none --pull=never`; use the exact already cached MariaDB digest
in the Containerfile. No Alpine switch, downloads, apt or hidden package install.

The base must already provide Python3 with Linux waitid/WNOWAIT, setpriv, mariadb, mariadb-install-db,
mariadbd, account helpers and mysql system identity. This has **not** been
verified by this source recipe for the selected digest. Missing pre-existing Python or a
helper is an explicit hard build blocker. No image/package success is claimed.

The required schema-administration interface is the explicit local-root CLI:

    podmesh-store-admin prepare --role node --profile ROOT_PROTECTED_PROFILE \
      --admin-socket PRIVATE_ROOT_ALIAS --admin-password-file ROOT0600_CREDENTIAL

Bind its immutable source revision and executable hash in the candidate manifest.
It must prepare only an empty role schema or verify the exact
current schema; the normal NODE must verify schema without DDL. An old binary
that attempts schema DDL fails under these grants. There is no compatibility
fallback, ambient DSN or SQLite fallback. Do not independently port other modules.

## New runtime envelope — native controller integration remains open

The eventual delivery owner must create **new** root-owned0755 persistent mounts
at `/apps/podmesh-node/etc` and `/apps/podmesh-node/sav`, with no unknown prior
data. They are separate declared mounts. `/apps`, the functional home and saved
ancestors remain root-owned/nonreplaceable. Only sav/app, runtime API/tmp directories
and etc/mysql/localhost/passwd belong to1102. Password0600 is generated once; its
parents stay root-owned. The mysql0700 datadir is sav/mariadb. Private root0700
administration/diagnostics/state is sav/admin and etc/admin; no app traversal.

The root filesystem/code must be immutable; `/run` must be a fresh root-owned
tmpfs per lifecycle. No stale runtime/maintenance readiness marker is accepted or
silently cleared. The DB socket directory is mysql:1102 mode0750; the socket is
mysql:1102 mode0660. A hardlink alias under root0700 on the **same /run filesystem**
provides the same socket inode to schema-admin; a symlink is not substituted.
Native alias/peer behavior must be tested on the build host and an isolated target.

Use `--image-volume=ignore` because the upstream image declares /var/lib/mysql.
Declare only etc, sav, fresh /run and the external host-capability directory.
No image VOLUME may create hidden storage. No host root, Podman/docker socket,
devices, host PID/IPC/network namespace or privileged container. Use private
namespaces/network-none, no published ports, memory/CPU/PID bounds measured for
the target. Set the external engine stop timeout to at least60s (the engine default
10s cannot accommodate the internal app/helper/DB graceful-stop bounds). Root bootstrap needs only CHOWN, DAC_OVERRIDE, FOWNER, SETUID, SETGID,
SETPCAP; drop the other capabilities. The delivery owner must verify this actual
envelope externally; no in-container claim proves host mount/namespace isolation.
App and DB children drop every capability set, supplemental group and privilege
gain using setpriv. Native delivery integration and effective capabilities remain
unqualified, so this is not an install/start/rollback replacement controller.

Mount `/run/podmesh-host` read-only as root:1102 mode0750 with a root-owned0660
capability.sock; the server must be root. The adapter stays a separate scoped host
boundary. No arbitrary host authority is granted to this container.

## Finite lifecycle and data authority

Fresh means empty mysql/app directories with no birth record, profile or credentials.
Initialization records intent before generating passwords or installing system
MariaDB tables. Partial/failed/unclean records refuse rather than reset/chown/adopt
unknown state. A clean restart requires matching datadir device/inode, credentials,
profile and artifact provenance. Restore/upgrade/import requires a later separate
explicit procedure; no in-place migration or automatic SQL copy exists here.

MariaDB starts with `--no-defaults --skip-networking --skip-name-resolve`, no binlog
or query logging. Same-container SQL readiness and socket peer/inode checks precede
schema preparation; observed TCP listeners refuse. Root authentication is local
Unix socket plus a separately generated native password, confined to root files.
Passwords enter SQL through private stdin/defaults files, never app env/argv or
public diagnostics. Administrative helper stderr is discarded (SQL errors may echo query fragments);
server/application diagnostics remain root-private, with server query logging disabled.

The root CLI runs before the app. Fresh grants are exact per-table DML, never
GRANT ALL: all40 tables SELECT; metadata/observations INSERT; operations and
operation_attempts INSERT+UPDATE; universe_volume_declarations INSERT+UPDATE;
secrets INSERT+UPDATE+DELETE for existing compensation/removal paths. No app DML
on store_schema/cutover ledger, DDL, schema-wide privilege or grant option.
SQLite-only domain tables receive no mutation grants. Restart verifies the exact account set, PUBLIC/inherited roles and
grant inventory and refuses role/global/schema/column/routine escalation instead
of repairing unknown permissions. Grant semantics require real MariaDB tests.

Application readiness requires the actual UID1102, empty supplementary groups,
zero capabilities, matching socket server PID/UID and a real capabilities response
identifying MariaDB. There is no readiness marker before that. PID1 watches both
owned children using non-reaping waitid/WNOWAIT until owned session cleanup ends; unexpected child exit is failure. TERM stops app before DB with
bounded10s/20s waits and KILL only for owned sessions if necessary. Init/schema/
SQL/readiness waits are finite. Forced stop or child failure exits nonzero and
retains state; clean stop is explicitly **not** a coherent backup receipt.

## Required affected proof

On the declared build host, after pinning the recipe and dependency: run the targeted
`python3 -m unittest discover -s packaging/podmesh-node/canonical-unit -p test_entrypoint.py`
(root for private-file/owned-child cases); validate Python syntax there; run the
schema-authority Rust tests when their source changes; verify base command/dependency and payload
hash gates before building the exact image offline. Passing these checks does not establish functional-unit runtime qualification.

Then, on a separately authorized disposable target, prove same-container app+DB
identities/noTCP/socket-only, readiness/TERM/crash ownership and failure propagation,
functional DML versus DDL denial, full native install/restart/rollback, actual
host adapter operation/replay, and conservation. Keep originals untouched.

Before **any data-bearing change**, retain a faithful coherent logical SQL copy,
coordinate declared non-reproducible volumes/config/image lock/provider bindings,
restore an isolated empty private target and independently compare useful data and
old replay plus a new operation. A blank DB beside file-authoritative state is not
compliance. No SQL dump/import, restore, retention, pruning or partition DDL is
implemented here; use inventory18 to select later table-specific UTC partitions
without weakening global operation IDs. No production or full-candidate claim.
