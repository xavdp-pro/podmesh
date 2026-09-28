# The store a node's journal is kept in

Every operation a PodMesh node performs is written into a journal, and the journal is what the next
operation reads to know what it may do. That journal has always been one SQLite file, `state.sqlite`,
under the node's state directory. [STORAGE-MARIADB-MIGRATION-PLAN.md](STORAGE-MARIADB-MIGRATION-PLAN.md)
moves it to a MariaDB instance owned by the node's functional role, and this page describes the
configuration surface that migration introduced, and what it does and does not do today.

**A node with no profile is a node whose behaviour is unchanged.** It opens `state.sqlite` under its
state directory, as it always has. What Phase 2 added is that the journal's *schema* is now one
ordered set of migrations applied at every open, on whichever engine the profile names, and that
`store.engine` is read: a node configured for MariaDB opens that instance and migrates it.

**What it does not yet do** is run from MariaDB. The node's operations still take a
`rusqlite::Connection`, so a node whose profile says `mariadb` opens the instance, brings its schema
up, and then refuses to serve, naming why. Failing closed is the point: a node that believes it
writes to a server and writes to a file instead is the one failure this layer must not allow.

## Where the profile is read from

A node reads its profile from `store.json` under its state directory (`/var/lib/podmesh/store.json`
by default), or from the file `PODMESH_STORE_PROFILE` names. No file means the default above. A file
that exists and cannot be read or parsed is a refusal at startup, never a return to the default: an
operator who wrote one is asking for it. The file holds the `store` object below, alone or inside a
larger document.

## The profile

```json
"store": {
  "engine": "sqlite",
  "sqlite": {
    "path": "/var/lib/podmesh/state.sqlite",
    "busy_timeout_ms": 5000,
    "journal_mode": "wal"
  },
  "mariadb": {
    "host": "127.0.0.1",
    "port": 3306,
    "socket": null,
    "user": "podmesh-node",
    "database": "podmesh-node",
    "password_file": "/apps/podmesh-node/etc/mysql/localhost/passwd",
    "connect_timeout_ms": 5000,
    "lock_wait_timeout_seconds": 10
  }
}
```

| Field | Means |
| --- | --- |
| `engine` | `sqlite` or `mariadb`. Anything else is refused by name, and `mariadb` in a build without the `mariadb` feature is refused rather than quietly opened as a file |
| `sqlite.path` | The file itself, not its directory. Absent, it is `state.sqlite` under the state directory, which is what a node has today |
| `sqlite.busy_timeout_ms` | How long a statement waits for another writer before the store reports the fault `busy` |
| `sqlite.journal_mode` | `wal`, as every node journal has used, or `default` |
| `mariadb.host`, `port`, `socket` | Where the instance is. The default is loopback, because a node's journal belongs to the node's own host; a socket is preferred over the address when both are given |
| `mariadb.user`, `database` | The node role's functional slug on both sides: system user, database user and database carry one name (`podmesh-node`), and the manager's store carries another (`podmesh-manager`) in its own instance |
| `mariadb.password_file` | The file the password is read from, mode `0600`. **A password is never a configuration value**: `store.mariadb.password` is refused outright |
| `mariadb.connect_timeout_ms` | How long a connection attempt waits. No read timeout is set: ending a long statement the node meant to run is a different decision |
| `mariadb.lock_wait_timeout_seconds` | `innodb_lock_wait_timeout` for this session. A row another transaction holds past it is reported `busy`, which is what a file another writer holds is reported as on SQLite |

Per the Phase 0 decision, the node store and the manager store never share an instance, a database or
credentials, even on a host that runs both.

### What an incomplete profile does

The profile is checked before anything is opened, and a MariaDB profile that does not say enough to
be reached is refused there:

| The profile | What happens |
| --- | --- |
| `engine` absent, or `sqlite` | The journal a node has today, at `sqlite.path` |
| `engine: mariadb`, no `password_file` and no `dsn` | Refused `denied`: a node's store credentials are read from a file, mode 0600 |
| `engine: mariadb`, a `password_file` that is empty or readable beyond its owner | Refused `denied`, naming the file and the mode it must have |
| `engine: mariadb`, no `user` or no `database`, or neither a `host` nor a `socket` | Refused `unavailable`, naming the field |
| `dsn` that is not `mysql://`, or names no database | Refused, with the password redacted out of the message |
| `engine: mariadb` in a build without the `mariadb` feature | Refused `unsupported`, by name, never opened as a file |

None of these leaves a `state.sqlite` behind.

## Building with MariaDB

The MariaDB backend is behind a Cargo feature, off by default, so the packaged `.deb` and any default
build keep the dependency set they have today:

```bash
cargo build --features mariadb
cargo test  --features mariadb
```

A build without the feature still reads and validates a `mariadb` profile; it refuses to *open* one,
by name, rather than falling back to SQLite. A node that believes it writes to MariaDB and writes to a
file instead is the one failure this layer must not allow.

## Running the MariaDB tests

The MariaDB tests skip when no server is named, and fail rather than skip when one is named and does
not open — an operator who names a server is asking for it to be used. Name it with a DSN:

```bash
PODMESH_MARIADB_DSN='mysql://podmesh-node:<password>@127.0.0.1:33061/podmesh-node' \
  cargo test --features mariadb store::mariadb
```

The DSN must name a database, and the tests write into it: they create, use and drop the tables whose
names begin with `store_` and, since Phase 2, the node's own tables. Point it at a store of its own,
never at one a node uses. The MariaDB tests take the server one at a time, because they share it.

A throwaway instance for a laboratory workstation, which is how these tests were first run:

```bash
podman run -d --name podmesh-store-test --network host \
  -e MARIADB_ROOT_PASSWORD="$PASSWORD" -e MARIADB_DATABASE=podmesh-node \
  -e MARIADB_USER=podmesh-node -e MARIADB_PASSWORD="$PASSWORD" \
  docker.io/library/mariadb:11 --bind-address=127.0.0.1 --port=33061
```

Host systemd or a throwaway container are for spikes and tests only. The product packaging is the
sidecar the Phase 0 record decided on, one instance per functional role.

## What the store contract gives a caller

`src/store/` carries the contract (`DurableStore`), the SQLite backend and the MariaDB backend. Two
conventions hold across both engines, because the dialects do not agree:

- **Placeholders are `?`, bound by position.** `?1` is SQLite's alone.
- **Values are SQLite's five storage classes** — null, integer, real, text, bytes. A MariaDB column is
  read back into the same five, so a caller written against one engine reads the same shapes on the other.

On every MariaDB open the backend sets session isolation to `REPEATABLE READ` and **refuses** to
open unless both `@@SESSION.innodb_flush_log_at_trx_commit` and `@@GLOBAL.innodb_flush_log_at_trx_commit`
are `1` (Muse counter-view, 2026-09-28). Durability is not a soft preference.

Faults are named rather than left as engine codes: `busy`, `locked`, `unavailable`, `denied`, `schema`,
`integrity`, `type`, `unsupported`, `other`. The pair that matters is `busy` and `locked` — the store
refused this attempt and the same attempt may succeed later. `SQLITE_BUSY`, `SQLITE_LOCKED`, MariaDB's
1205 (lock wait timeout) and 1213 (deadlock) all arrive as one of those two.

DDL is not portable and the layer does not pretend it is. `store_schema` is written in each engine's
own dialect, and the node's tables arrive the same way, as the migration set below.

## The node's schema, as migrations

`src/store/migrations/node/` holds the node's whole schema as numbered files applied in order:

```text
0001-base            metadata, observations
0002-lifecycle       operations, operation_attempts
0003-migration       the six live-migration tables
0004-activation      policy, leases, lease history, epochs, policy changes
0005-network         declaration, peer pools, allocations, routes, effect ledger
0006-secrets         secrets
0007-publisher       publishers, events, transitions, takeover proofs
0008-recovery-point  the eight recovery-point tables
0009-retention       retention, holds, retained points
0010-collector       collection runs and effects
```

That is the **38 production tables** the storage inventory counts. Each step is a pair of files,
`<id>.sqlite.sql` and `<id>.mariadb.sql`, because every `CREATE TABLE` in this set diverges: a
variable-length primary key needs a length on MariaDB and none on SQLite, so there is no portable
spelling of even the first table. A future step that is portable (an index, a row) may name one file
for both engines.

`store_schema` records the version: version *N* means the first *N* files have been applied. A store
recorded at a version this build does not have is refused rather than downgraded. Two rules hold for
every file, and a new one keeps them:

- **Every statement is idempotent.** MariaDB commits each DDL statement on its own, so a migration is
  not a transaction there whatever the engine promises elsewhere; a migration interrupted halfway is
  recovered by running it again.
- **A released migration is never edited.** The schema moves forward by adding a file, because a
  store recorded at version *N* will never run the first *N* files again.

The modules keep their `ensure_schema`: a journal written by an older package still needs the
`ALTER TABLE` upgrades they carry, and `CREATE TABLE IF NOT EXISTS` adds no column to a table that
already exists. Two tests keep the two in step — one asserts that a journal made by the migrations
has the same tables, columns, defaults, keys and indexes as one made by every module's
`ensure_schema`, and the other that running the modules against a migrated journal changes nothing.

### How the two dialects differ

| SQLite | MariaDB | Why |
| --- | --- | --- |
| `TEXT` as a primary or unique key | `VARCHAR(n)` | A key needs a bounded length here |
| `TEXT` elsewhere | `LONGTEXT` | Documents, digests and provenance are not indexed and are not truncated |
| `INTEGER` | `BIGINT` | Including the 0/1 booleans, so a caller reads back the integer it wrote |
| `INTEGER PRIMARY KEY` (rowid alias) | `BIGINT AUTO_INCREMENT` | The same "the store gives me the next one" |
| `... UNIQUE` on a column | a named `UNIQUE KEY` | A unique constraint needs the same bounded length |
| nothing declared | `ENGINE=InnoDB DEFAULT CHARSET=utf8mb4` | A store restored on another server carries the same character set and the same transactional engine |
| `key`, `at` unquoted | `` `key` ``, `` `at` `` | Reserved words here. Backticks are accepted by both, so one statement reads on both engines |
| `CREATE UNIQUE INDEX ... WHERE released_at IS NULL` | a virtual column plus a plain `UNIQUE KEY` | **The one construct with no equivalent.** MariaDB has no partial index, so `network_allocations` carries a virtual column that is the address while the allocation is live and NULL once it is released; a unique key ignores NULLs on both engines, so a released row stops taking part in uniqueness exactly as it does under the partial index |

## Asking what a store carries

`src/store/catalog.rs` answers *does this journal have that table* and *does that table have that
column* for both engines — `pragma_table_info` on one side, `information_schema` on the other — so
that no caller writes one engine's introspection. Every `ALTER TABLE` upgrade in the modules now asks
it, and `boot_restore` asks it whether a table exists.

Two SQLite-only escapes remain, and both are named in `catalog`'s own documentation rather than left
in a module:

- `catalog::connection::table_definition` returns the `CREATE TABLE` text SQLite kept, which is how
  `network::ensure_schema` recognises a `network_allocations` from the first two versions and rebuilds
  it. The question is what a table was *declared* with, and only SQLite keeps the declaration
  verbatim. A store the migration set made is never in the state this repairs.
- `tests/check-package-rehearsal.py` lists tables from `sqlite_master` while comparing a journal
  before and after a package operation. It reads the file directly, outside the node.
