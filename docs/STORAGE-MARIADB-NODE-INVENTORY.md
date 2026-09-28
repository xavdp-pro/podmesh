# Node SQLite storage inventory

Status: Phase 2 preparation at `1bc73fc`. This inventories production
`state.sqlite` DDL under `src/`; it is not a MariaDB qualification record.

## Open path

`src/lib.rs::open_state` is the only runtime open that owns and writes
`state.sqlite`. It creates the state directory, opens
`<state-dir>/state.sqlite`, enables WAL, and creates the base tables.
`src/bin/podmeshd.rs` calls it at daemon startup; `src/publisher.rs` also calls
it for connector startup. The default state directory remains
`/var/lib/podmesh`.

`src/bin/podmesh-storage-migrate.rs` opens the operator-supplied SQLite source
with `SQLITE_OPEN_READ_ONLY`; it neither creates schema nor opens the configured
runtime path implicitly.

The existing modules contain nine test-only in-memory opens: four in
`activation.rs`, one in `boot_restore.rs`, one in `network.rs`, and three in
`publisher.rs`. Their inline `CREATE TABLE` statements reproduce subsets of the
production tables for fixtures. The migration binary test has one temporary
file-backed open and creates `alpha` and `beta` fixtures. None adds a production
table.

## Production tables

There are **38 unique production tables**.

| Owning module | Tables | Migration risk |
| --- | --- | --- |
| `lib.rs` | `metadata`, `observations` | WAL setup is SQLite-only; `metadata` binds state to the machine identity. |
| `lifecycle.rs` | `operations`, `operation_attempts` | Operation idempotency and attempt history must retain transaction ordering; integer primary-key generation differs in MariaDB. |
| `migration.rs` | `migration_reservations`, `migration_authorizations`, `migration_restore_claims`, `migration_reservation_history`, `migration_universe_tombstones`, `migration_collection_history` | Unique operation/authorization bindings and retained tombstone/history semantics must survive the copy. |
| `activation.rs` | `activation_policy`, `activation_leases`, `activation_lease_history`, `activation_epochs`, `activation_policy_changes` | Runtime `ALTER TABLE` upgrades add authority, quorum, serial, epoch, grant, and boot fields; JSON-shaped values are stored as `TEXT`. |
| `network.rs` | `network_declaration`, `network_peer_pools`, `network_allocations`, `network_routes`, `network_effects` | Partial unique indexes enforce uniqueness only for live allocations; schema upgrade code reads `sqlite_master` and rebuilds `network_allocations`. |
| `secrets.rs` | `secrets` | Runtime PRAGMA inspection adds the transition-state column; rows contain metadata and digests, not secret values. |
| `publisher.rs` | `publishers`, `publisher_events`, `publisher_transitions`, `publisher_takeover_verified` | Runtime PRAGMA inspection adds quorum/digest fields; event IDs and the compound takeover key need equivalent MariaDB definitions. |
| `recovery_point.rs` | `recovery_points`, `recovery_point_restores`, `recovery_point_promotions`, `recovery_point_staged`, `recovery_point_live_captures`, `recovery_point_final_captures`, `recovery_point_live_promote_attempts`, `recovery_point_live_promotions` | Runtime PRAGMA inspection adds capture mode and uncompressed size; operation and recovery-point uniqueness guard replay. |
| `retention.rs` | `recovery_point_retention`, `collection_holds`, `recovery_point_retained` | Active holds and immutable retained-point records must keep nullable release fields and exact manifest text. |
| `collector.rs` | `garbage_collection_runs`, `garbage_collection_effects` | Append-only run records and per-candidate effect rows rely on atomic progress recording and a compound primary key. |

## Cross-cutting risks

- DDL is created lazily by module `ensure_schema` functions, without a schema
  version table or one ordered migration set.
- SQLite `INTEGER PRIMARY KEY`, partial indexes, `INSERT OR IGNORE` /
  `INSERT OR REPLACE`, PRAGMA introspection, and `sqlite_master` queries require
  explicit MariaDB equivalents.
- Timestamps and booleans are currently integer values; structured documents
  and digests are mostly `TEXT`. Type changes must preserve existing comparison
  and canonicalization behavior.
