-- Recovery points: the points this host prepared, what it restored, promoted or staged, and the
-- live (memory-coherent) capture and promotion records a crash is recovered from.
-- `capture` and `uncompressed_bytes` are the columns later versions appended by ALTER TABLE, with
-- the same defaults: a point made before live captures existed reads as a stopped capture.
CREATE TABLE IF NOT EXISTS recovery_points(
    recovery_point_uuid TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    generation INTEGER NOT NULL,
    parent_recovery_point_uuid TEXT,
    operation_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    rootfs_sha256 TEXT NOT NULL,
    rootfs_bytes INTEGER NOT NULL,
    prepared_at INTEGER NOT NULL,
    outbox TEXT NOT NULL,
    capture TEXT NOT NULL DEFAULT 'stopped');

CREATE TABLE IF NOT EXISTS recovery_point_restores(
    operation_id TEXT PRIMARY KEY,
    restored_universe_uuid TEXT NOT NULL UNIQUE,
    recovery_point_uuid TEXT NOT NULL,
    source_universe_uuid TEXT NOT NULL,
    imported_image_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    rootfs_sha256 TEXT NOT NULL,
    manifest_signed INTEGER NOT NULL,
    restored_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS recovery_point_promotions(
    operation_id TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    restored_universe_uuid TEXT NOT NULL,
    recovery_point_uuid TEXT NOT NULL,
    container_id TEXT NOT NULL,
    lease_generation INTEGER NOT NULL,
    promoted_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS recovery_point_staged(
    recovery_point_uuid TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    operation_id TEXT NOT NULL UNIQUE,
    generation INTEGER NOT NULL,
    archive_sha256 TEXT NOT NULL,
    archive_bytes INTEGER NOT NULL,
    manifest_sha256 TEXT NOT NULL,
    image_id TEXT NOT NULL,
    inbox TEXT NOT NULL,
    staged_at INTEGER NOT NULL,
    discarded_at INTEGER,
    discard_operation_id TEXT,
    promoted_operation_id TEXT,
    uncompressed_bytes INTEGER);

CREATE TABLE IF NOT EXISTS recovery_point_live_captures(
    operation_id TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    container_id TEXT NOT NULL,
    recovery_point_uuid TEXT NOT NULL,
    began_at INTEGER NOT NULL,
    state TEXT NOT NULL,
    detail TEXT);

CREATE TABLE IF NOT EXISTS recovery_point_final_captures(
    recovery_point_uuid TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    container_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    captured_at INTEGER NOT NULL,
    resumed_operation_id TEXT,
    resumed_at INTEGER);

CREATE TABLE IF NOT EXISTS recovery_point_live_promote_attempts(
    operation_id TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    recovery_point_uuid TEXT NOT NULL,
    lease_generation INTEGER NOT NULL,
    launched_at INTEGER NOT NULL,
    state TEXT NOT NULL,
    detail TEXT);

CREATE TABLE IF NOT EXISTS recovery_point_live_promotions(
    operation_id TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    recovery_point_uuid TEXT NOT NULL,
    container_id TEXT NOT NULL,
    lease_generation INTEGER NOT NULL,
    restore_log_sha256 TEXT NOT NULL,
    promoted_at INTEGER NOT NULL);
