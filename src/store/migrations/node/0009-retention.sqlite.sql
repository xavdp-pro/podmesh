-- Retention and collection: what an operator declared a universe must keep, the holds that refuse
-- collection while they are live, and the immutable record of every point actually collected.
CREATE TABLE IF NOT EXISTS recovery_point_retention(
    universe_uuid TEXT PRIMARY KEY,
    keep_latest INTEGER NOT NULL,
    minimum_age_seconds INTEGER NOT NULL,
    declared_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    authorization_ref TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS collection_holds(
    hold_id TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    scope TEXT NOT NULL,
    reason TEXT NOT NULL,
    declared_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    authorization_ref TEXT NOT NULL,
    released_at INTEGER,
    released_by_operation TEXT,
    release_authorization_ref TEXT);

CREATE TABLE IF NOT EXISTS recovery_point_retained(
    recovery_point_uuid TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    generation INTEGER NOT NULL,
    path_class TEXT NOT NULL,
    manifest TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    rootfs_sha256 TEXT NOT NULL,
    rootfs_bytes INTEGER NOT NULL,
    prepare_operation_id TEXT NOT NULL,
    terminal_state TEXT NOT NULL,
    collected_at INTEGER NOT NULL,
    collecting_operation_id TEXT NOT NULL,
    retention TEXT NOT NULL);
