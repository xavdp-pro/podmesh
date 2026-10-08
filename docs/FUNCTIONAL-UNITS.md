# PodMesh functional units — SHAPER OS alignment

Profile: **SEP22-CONTAINER-MARIADB** (SHAPER OS V1.15 @
`173ae591988e1f71a33f65289fc26b9de4aaf3d7`).
Rules 4 and 26. Operator direction:
[24 September 2026 functional-unit MariaDB](https://github.com/xavdp-pro/SHAPER-OS-V1.15/blob/173ae591988e1f71a33f65289fc26b9de4aaf3d7/decisions/2026-09-24-FUNCTIONAL-UNIT-MARIADB.md).

Does **not** turn PodMesh into a SHAPER universe (Vault / Logger / Queue / Maestro).

## Declared units

| Functional slug | Responsibility | Private MariaDB |
| --- | --- | --- |
| `podmesh-node` | Per-host journal and typed local operations | Own **server instance**; user = database = `podmesh-node` |
| `podmesh-manager` | Replicated manager history and active-manager facts | Own **server instance**; slug `podmesh-manager` |

Credentials follow the slug. Application rights stay inside that database.

## Rules

1. Node and manager never share instance, database, or credentials.
2. SQLite is scaffolding until cutover, then debt — not conformity evidence.
3. Done for each unit: `mariadb-dump` → empty instance → restore → integrity check.
4. Workstation HA gate files are not units; do not migrate them to MariaDB.

## Product shape and identities

The candidate shape is one isolated Podman runtime boundary per declared unit.
Each boundary contains its application and its private MariaDB server, with owned
persistent application and database volumes. Node and manager are independently
installable functions; sharing a host does not merge their identity or lifecycle.
A database sidecar outside the owning unit boundary is not this profile.

Inside each unit, `functional slug = application system user = DB user = database`.
The application account has only its database rights. Database administration and
bootstrap credentials use a separate authorized path and never become runtime
application credentials. Preserve identity, profile, image digest/lock, database
and declared volume state through restoration; a new empty database is not recovery.

The host adapter is a separate privileged execution boundary. It exposes bounded
host capabilities (rootful Podman and explicitly declared storage/network actions)
under a typed request and mandate; it does not own the application database or
receive unrestricted application authority. The application records operation
identity and intent before requesting an effect and records the observed outcome.
Privileged execution cannot widen a human/parent mandate or grant Governor/Maker
power. The existing host-root `podmeshd` packaging is a transitional implementation,
not proof of this separation or of an isolated functional unit.

## Source interfaces for private candidates

The [node recipe](../packaging/podmesh-node/private-unit/README.md) uses actual
application UID/GID1102 and its own private server. Separate `podmesh-host-adapter`
owns root policy and retained host identity records; boot facts/lifecycle cross that
typed boundary. Nested grants bind exact UUID/base image and explicitly declared
empty host mounts, namespace and privileged device envelope, not inner continuity.
The [manager recipe](../packaging/podmesh-manager/private-unit/README.md) uses
application UID/GID1103 and a separate private server; network/resident resolve the
same backend with no SQLite fallback. These are candidate inputs, not complete
installers or runtime qualification. Application containers suppress inherited
image volumes; only declared owned app/DB volumes belong to the boundary. The
[node proof scenario](../packaging/podmesh-node/qualification/README.md) leaves
host/VM/recovery actions to the separately authorized runtime owner.

## Delivery and proof

1. Commit an immutable source candidate and build/package those bytes on an explicit
   build host. Runtime qualification uses that manifest and those artifact hashes.
2. Port the required node operations through `DurableStore`, preserving ordering,
   replay, ownership and refusal gates. Unsupported modules refuse by name without
   opening a SQLite fallback.
3. Qualify each private store with an actual product operation/journal, dump,
   empty-target restore, integrity and a nominal functional result. A controlled
   SQL fixture proves engineering behavior only.
4. Qualify install/start/stop and rollback for the same pin. Standalone qualification
   does not establish SHAPER integration: that additionally needs Maker recipe,
   Logger/parent evidence and typed verification of failures and escalation.

Current operation coverage and boundaries: [STORE-CONFIGURATION.md](STORE-CONFIGURATION.md).
Release perimeter and laboratory progress belong in the private workshop, not in
branch names or host defaults in this contract.

## Config names

[STORE-CONFIGURATION.md](STORE-CONFIGURATION.md) — keep `podmesh-node` /
`podmesh-manager` identical to the functional slug.
