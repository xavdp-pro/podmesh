# Manager store inventory (MariaDB migration)

The manager replica journal (`manager.sqlite` today) carries **four** production tables,
created by `src/store/migrations/manager/` and recorded in `store_schema` under the name
`manager`. Legacy journals use SQLite `PRAGMA user_version = 3` with no `store_schema` row;
`podmesh-storage-migrate --role manager` maps that to the current migration version.

| Table | Role |
| --- | --- |
| `identity` | Singleton row: replica id and topology JSON |
| `facts` | Append-only fact log |
| `receipts` | Operation receipts (immutable) |
| `exchange_audit_events` | Wire exchange audit trail (immutable) |

Offline cutover uses `store_schema.manager_cutover = 0` until
`podmesh-storage-migrate --role manager` finishes copy and verification; until then
`open_manager_store` refuses the target.
