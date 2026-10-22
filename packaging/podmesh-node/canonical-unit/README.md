# Canonical NODE unit — unbuilt, unqualified source candidate

This **new** recipe is a single container: root PID1 supervises MariaDB as mysql
and the exact NODE binary as Linux UID/GID1102, system user/database user/database
`podmesh-node`. It replaces no installed unit and does not alter the old
`private-unit`/`delivery` recipes or their scoped historical PASS. PodMesh is not
a five-service SHAPER core universe. Product adoption remains SHAPER OS
`173ae591988e1f71a33f65289fc26b9de4aaf3d7`; the current operator's Rules4/26 and
Turbinobash direction define the target, not a new conformity claim.

## Required immutable inputs and dependency gates

Build context: this Containerfile, entrypoint.py, grants.json, the exact
`podmeshd` and `podmesh-store-admin` binaries, install-python-closure.sh and the
collected bundle at `python-closure/`. Mandatory SHA256/source/recipe pins bind
all existing payloads; PYTHON_CLOSURE_SHA256 and PYTHON_INSTALLER_SHA256 additionally
bind the authenticated dependency lock and offline installer. The same immutable
MariaDB digest remains selected. No Alpine switch or arbitrary host Python tree.
Build with `--network=none --pull=never`; image construction uses verified local
Debian packages only. Dependency collection is an explicit separate build action.

### Authenticated Python dependency closure — source implementation, unqualified

Only uncompressed UTF-8 deb822 Packages indices are supported. The collector sets
`Acquire::GzipIndexes=false` and `Acquire::IndexTargets::deb::Packages::KeepCompressed=false`
and obtains actual filenames/URI/component/architecture from `apt-get indextargets`.
The target must also report KeepCompressed=no and the exact configured Release;
compressed sibling files are rejected. It does not guess Packages cache filenames or equate compressed bytes with a signed
uncompressed hash. Unknown encoding, compressed retained targets, missing index
or conflicting source/candidate metadata refuses without fallback.

For each exact official source/suite, gpgv verified plaintext and externally bound
archive primary signers authenticate Release; signing-subkey fingerprints are
retained separately. Strict gpgv success is required: extra unknown, expired, revoked
or bad signatures refuse, with no permissive multiple-signature fallback. Its SHA256/size entry for
`main/binary-amd64/Packages` must match a private streamed snapshot of the actual
index. Selected package fields and retained raw stanzas come from those matched
bytes, never apt-cache self-report. Exact artifact matches mirrored across security
and updates retain all identical-artifact provenance; conflicting identities or
repeated selected rows inside one index refuse. Retained signed files, key, binding,
index snapshots, signer/plaintext hashes and selected stanzas allow verify_bundle
to replay signature verification and the complete index/artifact chain offline.
Verification uses a separate private temporary scratch directory, never writes
inside retained bundles, and checks input sizes before reading. Package/evidence
ordering is canonical by name; actual download URIs must match authenticated
Filename fields and source origins before download.
Synthetic tests mock only the signature boundary and do not prove real OpenPGP.
Real signed metadata collection and an offline image build remain required.

APT's [index-target documentation](https://github.com/Debian/apt/blob/main/doc/acquire-additional-files.md)
defines deb822 indextargets and KeepCompressed=false. The implementation intentionally
supports that narrow uncompressed retention contract, not every APT cache encoding.

`prepare-python-closure.py` is a build tool, not a runtime initializer. It takes
explicit absolute `--base-root`, `--base-binding`, `--base-binding-sha256`, absent
`--work` and `--output`. Parents must be private/protected and paths portable.
Never infer an image identity from a root directory name. The separate build
acceptance supplies a hash-pinned `podmesh.base-root-binding/v1` JSON document:
`base` has registry/image/archive IDs matching this recipe; ubuntu_archive_fingerprints
has the externally verified archive PRIMARY key fingerprints; suite=noble,
arch=amd64, verification=oci-config-diffids-checked; `files` has SHA256s for
os_release, dpkg_status, ubuntu_key and dpkg_elf. The authority issuing that
binding must first verify the retained OCI config/rootfs chain. The collector
only verifies consistency with that externally trusted binding; it does not
self-certify the rootfs. Image absolute symlinks resolve inside the image root,
never onto the build host. Actual Ubuntu24.04 noble, amd64 ELF and installed
package metadata are checked before creating work.

The collector uses a fresh isolated APT_CONFIG, etc/state/cache/status/log/key/
source/lock paths and sanitized environment. Host APT methods/binaries are
read-only tools; host package status/config/hooks are never used or modified.
APT acquisition uses the existing unprivileged _apt identity. Pre-existing
searchable protected work ancestors are an explicit prerequisite. Linux Python
stdlib FD xattrs encode POSIX ACCESS ACLs directly; no setfacl/libacl, package
installation, root acquisition fallback or parent ACL change is used. The caller
must exclusively own the newly constructed tree. Before any mutation, held
O_NOFOLLOW descriptors verify the exact 24-node layout, root ownership, private
0700/0600 modes, inode/name/mount identity, single-linked files, absent ACCESS and
default ACLs, and finite node/depth/byte bounds. Symlinks, foreign bind mounts,
unexpected nodes and inherited ACLs refuse collection. Privileged concurrent
writers are outside this exclusive-construction contract.

Only _apt receives r-x on directories and r-- on files. GROUP_OBJ and OTHER are
zero; the ACL MASK equals named-user rights (stat group bits reflect that mask,
not group membership access). The two partial acquisition directories instead
transfer to the existing _apt UID/GID at 0700 without ACLs. FD xattr bytes and
metadata are read back before APT; unsupported/denied xattrs, identity changes or
readback mismatches stop collection. XATTR_CREATE is not treated as an atomic
absence guarantee; the checks rely on exclusive ownership of the new tree. Any
partial setup is retained within the new tree, with no acquisition child started. No host parent or global APT path is
modified. Work/output remain private to their designated identities.

The fixed little-endian ACL ABI is version 2 with USER_OBJ, named USER, GROUP_OBJ,
MASK, OTHER entries from the Linux UAPI
[posix_acl_xattr.h](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/posix_acl_xattr.h)
and [posix_acl.h](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/posix_acl.h).
Targeted contract fixtures: `python3 -m unittest discover -s packaging/podmesh-node/canonical-unit -p test_python_closure.py -k NativePrivateAPTACL -v`.
These mock xattrs and require authorized Linux root fixture execution; they do
not prove kernel permissions. A separately authorized CT can opt into actual
kernel ACL/access checks with `PODMESH_RUN_REAL_APT_ACL_TESTS=1` and the same
command using `-k RealPrivateAPTACL`. That check drops supplementary groups and
compares _apt access against same-group, root-group and unrelated identities,
including denied file/unlink/outside writes and permitted private partial writes.

Only official Ubuntu HTTPS noble/main, updates and security sources signed with
the image's Ubuntu archive key are selected. Signature and expiry validation stay
enabled; signed Release origin/suite/arch and package metadata are recorded.
Installed base versions are pinned and any proposed upgrade/removal/reinstall is
rejected. Resolution uses the full `python3` distribution and libpython3-stdlib,
not python3-minimal alone; current versions come from signed metadata.

Only update metadata, simulate, print-URIs, query metadata and fixed download-only
commands run on the build host. No host install/unpack/configure. Finite time,
metadata/output/download/package-count bounds and measured filesystem bytes gate
work; failures retain work. Local .debs are matched to signed SHA256/size/name/
version/arch before publication. The output retains closure.lock, exact artifacts,
key/signed metadata and evidence.json. Its deterministic lock is data, not shell;
it pins actual resolved versions, every artifact and the external base binding.
Future collection can resolve different official versions; the lock/artifacts
are the reproducible input to rebuild that exact selected closure.

The offline image installer verifies the trusted lock hash, all listed bytes,
package identities and absence of extra/symlink artifacts BEFORE installation.
It also verifies the unmodified base status/os-release/key bytes. A fresh empty
image-local APT configuration plus --no-download/--no-upgrade/--no-remove selects
only exact local packages and preserves dependency/Pre-Depends ordering; service
autostart is denied during construction and original image policy restored.
Installed expected versions and actual hashlib/json/pathlib/secrets/socket/
subprocess/tempfile/unittest imports plus Linux WNOWAIT are verified afterwards.
No image success or runtime compatibility is inferred until that stage passes.

Other setpriv/SQL/server/account helpers and mysql identity remain mandatory
pre-existing base inputs. No missing helper triggers a network install. Root
administrative paths, engine/authentication and existing supervisor remain unchanged.

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
schema-authority Rust tests when their source changes; run only the new
`test_python_closure.py` cases for closure changes; retain unaffected supervisor proofs; verify base command/dependency and payload
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
implemented here; use the table-specific data contract to select later UTC partitions
without weakening global operation IDs. No production or full-candidate claim.
