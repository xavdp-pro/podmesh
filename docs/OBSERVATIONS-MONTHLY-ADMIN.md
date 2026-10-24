# Explicit monthly UTC observations administration

`podmesh-store-admin` has an optional NODE-only constructor. Use it only for a
new empty disposable canonical NODE database, with the existing private root
Unix socket, separate root credential and root-owned functional profile. The
application remains a DML-only account. No functional startup schedules DDL.

A protected root-owned JSON plan contains exactly:

```json
{"first_month":"2026-10","months":2}
```

The month is a UTC calendar month, not the host timezone or current date. Supported
first months are 1970-01 through 9998-12; the final bound cannot exceed 9999-01.
The sole count ceiling derives from MariaDB's 8192-partition maximum, counting
legacy and future. It is neither a sizing recommendation nor a retention period.

The existing root CLI gains these explicit forms (all paths are deployment inputs):

```text
podmesh-store-admin prepare --role node --profile ROOT_PROFILE \
  --admin-socket PRIVATE_UNIX_SOCKET --admin-password-file ROOT0600_FILE \
  --observations-months ROOT_PLAN_JSON
podmesh-store-admin advance-observations --role node --profile ROOT_PROFILE \
  --admin-socket PRIVATE_UNIX_SOCKET --admin-password-file ROOT0600_FILE \
  --observations-months ROOT_NEXT_PLAN_JSON
```

The constructor refuses every preexisting table, including an empty observations
table. It creates observations before running the unchanged numbered migrations.
The base migration's `CREATE TABLE IF NOT EXISTS` preserves that table. It never
edits a released migration, converts an old database, copies rows or changes a
schema version. Every other NODE table keeps the exact ordinary migration DDL.

Only observations uses `PRIMARY KEY(id, observed_at)` and `RANGE(observed_at)`.
The current node generates `observed_at` as integer UNIX epoch seconds. SQL gets
precomputed integer UTC boundaries, with no `UNIX_TIMESTAMP`, local timezone,
clock-derived expression or generated month column. `p_legacy` accepts every
value below the first month, including negative timestamps; each `pYYYYMM` covers
one half-open month; `p_future` covers the final bound and everything after it.
Late arrival and arbitrarily distant future timestamps are accepted.

`id` remains AUTO_INCREMENT and is first in the primary key. Production writes
omit it and readers order by it as a journal cursor. No current source references
an observation ID as a business identity. Composite uniqueness deliberately
permits an explicit duplicate id with another observed_at; callers must not
introduce such writes or references. Operations, attempts, restore claims,
tombstones, network uniqueness and other safety ledgers keep their global keys.
This does not claim that partitioning makes existing ORDER BY id readers faster.

Maintenance requires an exact-current NODE and recognizes only the constructor's
key and contiguous UTC partition layout. The next plan must begin at the existing
future partition's lower bound. Root issues one `REORGANIZE PARTITION p_future`
that adds the requested months and preserves a MAXVALUE catch-all. Existing late
ranges and all rows remain. Already stored future rows move into their appropriate
new partitions. Repeating a stale plan is refused, rather than resetting history.
There is no purge, DROP PARTITION, retention deletion, scheduler or in-place
conversion path. Total partition count is checked before maintenance.

Both commands retain the existing offline connection inventory and private-root
authentication guards. Offline operation also requires the operator to quiesce the
unit and exclude new connections for the whole action: a connection snapshot is
not a server-wide lock. DDL autocommits; an interrupted constructor or failed
postcondition preserves state and refuses automatic repair. A later authorized
recovery decision is required. A populated future partition may require data
movement and metadata locks; no latency/capacity promise is made.

The default `prepare` behavior, images, entrypoints and grants are unchanged.
There is no environment variable that opts an ordinary image into partitioning.
A root-controlled fresh-database preparation phase is required before ordinary
application startup. This optional source constructor is not a completed image
integration or adoption of a new default.

New planner/parser cases and an ignored native MariaDB integration case accompany
this change. The native case requires a separately leased empty disposable NODE
instance with an existing functional account and the exact current DML grant
manifest. Set `PODMESH_MONTHLY_TEST_SCOPE=EMPTY_DISPOSABLE_CANONICAL_NODE` and
explicit `PODMESH_MONTHLY_TEST_PROFILE`, `PODMESH_MONTHLY_TEST_ADMIN_SOCKET`,
`PODMESH_MONTHLY_TEST_ADMIN_PASSWORD_FILE`, `PODMESH_MONTHLY_TEST_RECORD_DIR`
(existing empty protected root0700 directory), then explicitly select the ignored
`monthly_native_root_constructor_boundary_dml_rollover_and_global_id` test after
building the adopted immutable source with the `mariadb` feature on its CT.
It retains data and private evidence, including intentional refusals. It checks
root construction, DML at boundaries/late/future times, DDL denial, populated
catch-all reorganization, exact retained rows and global operation-ID uniqueness.
It does not execute a Podman lifecycle request, qualify full replay behavior, or
qualify partition-preserving logical transfer. Those exact native post-adoption
checks remain required before any monthly-image/runtime qualification. Previous
unpartitioned logical-transfer evidence cannot establish partition preservation.

Official SQL references:
[partition limitations](https://mariadb.com/docs/server/server-usage/partitioning-tables/partitioning-limitations),
[RANGE and MAXVALUE](https://mariadb.com/docs/server/server-usage/partitioning-tables/partitioning-types/range-partitioning-type),
[maintenance and AUTO_INCREMENT](https://mariadb.com/docs/server/server-usage/partitioning-tables/partition-maintenance).
