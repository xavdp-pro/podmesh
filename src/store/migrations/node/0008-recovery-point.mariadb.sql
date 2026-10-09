-- Recovery points: the points this host prepared, what it restored, promoted or staged, and the
-- live (memory-coherent) capture and promotion records a crash is recovered from.
-- `manifest_signed` is a boolean written as 0 or 1, kept as BIGINT so it is read back as the same
-- integer the SQLite journal returns rather than as a narrower type a caller would have to know.
CREATE TABLE IF NOT EXISTS recovery_points(
    recovery_point_uuid VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    generation BIGINT NOT NULL,
    parent_recovery_point_uuid VARCHAR(128) NULL,
    operation_id VARCHAR(128) NOT NULL,
    state VARCHAR(64) NOT NULL,
    manifest_sha256 VARCHAR(64) NOT NULL,
    rootfs_sha256 VARCHAR(64) NOT NULL,
    rootfs_bytes BIGINT NOT NULL,
    prepared_at BIGINT NOT NULL,
    outbox LONGTEXT NOT NULL,
    capture VARCHAR(32) NOT NULL DEFAULT 'stopped',
    PRIMARY KEY (recovery_point_uuid),
    UNIQUE KEY recovery_points_operation (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_restores(
    operation_id VARCHAR(128) NOT NULL,
    restored_universe_uuid VARCHAR(64) NOT NULL,
    recovery_point_uuid VARCHAR(128) NOT NULL,
    source_universe_uuid VARCHAR(64) NOT NULL,
    imported_image_id VARCHAR(128) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    manifest_sha256 VARCHAR(64) NOT NULL,
    rootfs_sha256 VARCHAR(64) NOT NULL,
    manifest_signed BIGINT NOT NULL,
    restored_at BIGINT NOT NULL,
    PRIMARY KEY (operation_id),
    UNIQUE KEY recovery_point_restores_universe (restored_universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_promotions(
    operation_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    restored_universe_uuid VARCHAR(64) NOT NULL,
    recovery_point_uuid VARCHAR(128) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    lease_generation BIGINT NOT NULL,
    promoted_at BIGINT NOT NULL,
    PRIMARY KEY (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_staged(
    recovery_point_uuid VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    generation BIGINT NOT NULL,
    archive_sha256 VARCHAR(64) NOT NULL,
    archive_bytes BIGINT NOT NULL,
    manifest_sha256 VARCHAR(64) NOT NULL,
    image_id VARCHAR(128) NOT NULL,
    inbox LONGTEXT NOT NULL,
    staged_at BIGINT NOT NULL,
    discarded_at BIGINT NULL,
    discard_operation_id VARCHAR(128) NULL,
    promoted_operation_id VARCHAR(128) NULL,
    uncompressed_bytes BIGINT NULL,
    PRIMARY KEY (recovery_point_uuid),
    UNIQUE KEY recovery_point_staged_operation (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_live_captures(
    operation_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    recovery_point_uuid VARCHAR(128) NOT NULL,
    began_at BIGINT NOT NULL,
    state VARCHAR(64) NOT NULL,
    detail LONGTEXT NULL,
    PRIMARY KEY (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_final_captures(
    recovery_point_uuid VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    captured_at BIGINT NOT NULL,
    resumed_operation_id VARCHAR(128) NULL,
    resumed_at BIGINT NULL,
    PRIMARY KEY (recovery_point_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_live_promote_attempts(
    operation_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    recovery_point_uuid VARCHAR(128) NOT NULL,
    lease_generation BIGINT NOT NULL,
    launched_at BIGINT NOT NULL,
    state VARCHAR(64) NOT NULL,
    detail LONGTEXT NULL,
    PRIMARY KEY (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_live_promotions(
    operation_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    recovery_point_uuid VARCHAR(128) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    lease_generation BIGINT NOT NULL,
    restore_log_sha256 VARCHAR(64) NOT NULL,
    promoted_at BIGINT NOT NULL,
    PRIMARY KEY (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
