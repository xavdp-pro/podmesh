-- Garbage collection: the append-only record of every run, and one row per candidate the run
-- acted on. Each effect is recorded in the same transaction as the effect itself, so a run
-- interrupted between an effect and its report recovers what it did instead of doing it twice.
CREATE TABLE IF NOT EXISTS garbage_collection_runs(
    operation_id TEXT PRIMARY KEY,
    mode TEXT NOT NULL,
    authorization_ref TEXT NOT NULL,
    collector_version TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER NOT NULL,
    record TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS garbage_collection_effects(
    operation_id TEXT NOT NULL,
    candidate_key TEXT NOT NULL,
    class TEXT NOT NULL,
    universe_uuid TEXT NOT NULL,
    applied_at INTEGER NOT NULL,
    result TEXT NOT NULL,
    PRIMARY KEY(operation_id, candidate_key));
