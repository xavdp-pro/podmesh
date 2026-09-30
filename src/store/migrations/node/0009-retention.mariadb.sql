-- Retention and collection: what an operator declared a universe must keep, the holds that refuse
-- collection while they are live, and the immutable record of every point actually collected.
-- `manifest` keeps the exact manifest text a retained point was collected with; it is a document,
-- not a key, so it is LONGTEXT and never indexed.
CREATE TABLE IF NOT EXISTS recovery_point_retention(
    universe_uuid VARCHAR(64) NOT NULL,
    keep_latest BIGINT NOT NULL,
    minimum_age_seconds BIGINT NOT NULL,
    declared_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS collection_holds(
    hold_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    scope VARCHAR(64) NOT NULL,
    reason LONGTEXT NOT NULL,
    declared_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    released_at BIGINT NULL,
    released_by_operation VARCHAR(128) NULL,
    release_authorization_ref LONGTEXT NULL,
    PRIMARY KEY (hold_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS recovery_point_retained(
    recovery_point_uuid VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    generation BIGINT NOT NULL,
    path_class VARCHAR(64) NOT NULL,
    manifest LONGTEXT NOT NULL,
    manifest_sha256 VARCHAR(64) NOT NULL,
    rootfs_sha256 VARCHAR(64) NOT NULL,
    rootfs_bytes BIGINT NOT NULL,
    prepare_operation_id VARCHAR(128) NOT NULL,
    terminal_state VARCHAR(64) NOT NULL,
    collected_at BIGINT NOT NULL,
    collecting_operation_id VARCHAR(128) NOT NULL,
    retention LONGTEXT NOT NULL,
    PRIMARY KEY (recovery_point_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
