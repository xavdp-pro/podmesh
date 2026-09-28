# The store a node's journal is kept in

Every operation a PodMesh node performs is written into a journal, and the journal is what the next
operation reads to know what it may do. That journal has always been one SQLite file, `state.sqlite`,
under the node's state directory. [STORAGE-MARIADB-MIGRATION-PLAN.md](STORAGE-MARIADB-MIGRATION-PLAN.md)
moves it to a MariaDB instance owned by the node's functional role, and this page describes the
configuration surface that migration introduced, and what it does and does not do today.

**Today it changes nothing.** A node opens `state.sqlite` exactly as it did before, no operation reads
`store.engine`, and a node with no `store` object in its configuration is a node whose behaviour is
unchanged. What exists is the abstraction (`src/store/`, Phase 1) and the profile below, so that
Phase 2 can move the node's schema and its call sites one at a time rather than all at once.

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

The DSN must name a database, and the tests write into it: they create, use and drop tables whose
names begin with `store_`. Point it at a store of its own, never at one a node uses.

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

Faults are named rather than left as engine codes: `busy`, `locked`, `unavailable`, `denied`, `schema`,
`integrity`, `type`, `unsupported`, `other`. The pair that matters is `busy` and `locked` — the store
refused this attempt and the same attempt may succeed later. `SQLITE_BUSY`, `SQLITE_LOCKED`, MariaDB's
1205 (lock wait timeout) and 1213 (deadlock) all arrive as one of those two.

DDL is not portable and the layer does not pretend it is. The only schema it installs is its own
`store_schema` table, written once per engine, which records which schema a store carries and at which
version. Phase 2 brings the node's tables as versioned migrations with the engine-specific sections
the plan calls for.
