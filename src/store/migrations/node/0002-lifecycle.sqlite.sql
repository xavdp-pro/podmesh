-- The operation journal: one row per operation ID, and the attempts made under it.
-- operation_attempts is a separate table so that an experimental3 rollback, which inserts four
-- values into operations, keeps working on a journal written by this version.
CREATE TABLE IF NOT EXISTS operations(
    id TEXT PRIMARY KEY,
    request TEXT NOT NULL,
    status TEXT NOT NULL,
    result TEXT);

CREATE TABLE IF NOT EXISTS operation_attempts(
    id INTEGER PRIMARY KEY,
    operation_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    outcome TEXT,
    detail TEXT);
