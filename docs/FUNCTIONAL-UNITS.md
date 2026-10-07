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

## Order

1. Cut over **`podmesh-node`** on `cursor/assembly-2026-10-07`.
2. Then **`podmesh-manager`**.
3. Prove restore for each before claiming Rule 26 for that unit.

Product shape: one MariaDB server instance per slug (not several databases on one shared server).

## Config names

[STORE-CONFIGURATION.md](STORE-CONFIGURATION.md) — keep `podmesh-node` /
`podmesh-manager` identical to the functional slug.
