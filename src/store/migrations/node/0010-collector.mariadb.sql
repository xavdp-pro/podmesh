-- Garbage collection: the append-only record of every run, and one row per candidate the run
-- acted on. Each effect is recorded in the same transaction as the effect itself, so a run
-- interrupted between an effect and its report recovers what it did instead of doing it twice.
-- The compound key stays inside this engine's index limit: (128 + 191) characters of utf8mb4.
CREATE TABLE IF NOT EXISTS garbage_collection_runs(
    operation_id VARCHAR(128) NOT NULL,
    mode VARCHAR(64) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    collector_version VARCHAR(64) NOT NULL,
    policy_version VARCHAR(64) NOT NULL,
    started_at BIGINT NOT NULL,
    finished_at BIGINT NOT NULL,
    record LONGTEXT NOT NULL,
    PRIMARY KEY (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS garbage_collection_effects(
    operation_id VARCHAR(128) NOT NULL,
    candidate_key VARCHAR(191) NOT NULL,
    class VARCHAR(64) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    applied_at BIGINT NOT NULL,
    result LONGTEXT NOT NULL,
    PRIMARY KEY (operation_id, candidate_key)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
