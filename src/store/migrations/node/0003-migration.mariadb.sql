-- Live migration: the reservation a universe is held under, the transfer authorization issued for
-- it, the destination's restore claim, and the history and tombstones those leave behind.
-- `operation_id TEXT NOT NULL UNIQUE` is a named UNIQUE KEY here: a unique constraint on a
-- variable-length column needs a bounded length on this engine.
CREATE TABLE IF NOT EXISTS migration_reservations(
    universe_uuid VARCHAR(64) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    image_id VARCHAR(128) NOT NULL,
    source_host_uuid VARCHAR(64) NOT NULL,
    destination_host_uuid VARCHAR(64) NOT NULL,
    container_started_at VARCHAR(64) NOT NULL,
    state VARCHAR(64) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    detail LONGTEXT NULL,
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS migration_authorizations(
    authorization_id VARCHAR(128) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    checkpoint_operation_id VARCHAR(128) NOT NULL,
    destination_host_uuid VARCHAR(64) NOT NULL,
    handoff LONGTEXT NOT NULL,
    handoff_sha256 VARCHAR(64) NOT NULL,
    state VARCHAR(64) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    outcome LONGTEXT NULL,
    outcome_sha256 VARCHAR(64) NULL,
    completed_by_operation VARCHAR(128) NULL,
    PRIMARY KEY (authorization_id),
    UNIQUE KEY migration_authorizations_operation (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS migration_restore_claims(
    authorization_id VARCHAR(128) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    handoff LONGTEXT NOT NULL,
    handoff_sha256 VARCHAR(64) NOT NULL,
    source_host_uuid VARCHAR(64) NOT NULL,
    source_container_id VARCHAR(128) NOT NULL,
    image_id VARCHAR(128) NOT NULL,
    state VARCHAR(64) NOT NULL,
    container_id VARCHAR(128) NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    outcome LONGTEXT NULL,
    outcome_sha256 VARCHAR(64) NULL,
    detail LONGTEXT NULL,
    PRIMARY KEY (authorization_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS migration_reservation_history(
    id BIGINT NOT NULL AUTO_INCREMENT,
    universe_uuid VARCHAR(64) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    image_id VARCHAR(128) NOT NULL,
    source_host_uuid VARCHAR(64) NOT NULL,
    destination_host_uuid VARCHAR(64) NOT NULL,
    container_started_at VARCHAR(64) NOT NULL,
    state VARCHAR(64) NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    detail LONGTEXT NULL,
    archived_at BIGINT NOT NULL,
    archived_by_operation VARCHAR(128) NOT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS migration_universe_tombstones(
    universe_uuid VARCHAR(64) NOT NULL,
    class VARCHAR(64) NOT NULL,
    class_number BIGINT NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    container_absent_at_collection BIGINT NOT NULL,
    checkpoint_operation_id VARCHAR(128) NOT NULL,
    collected_by_operation VARCHAR(128) NOT NULL,
    collected_at BIGINT NOT NULL,
    proof LONGTEXT NOT NULL,
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS migration_collection_history(
    id BIGINT NOT NULL AUTO_INCREMENT,
    universe_uuid VARCHAR(64) NOT NULL,
    class VARCHAR(64) NOT NULL,
    class_number BIGINT NOT NULL,
    container_id VARCHAR(128) NOT NULL,
    container_absent_at_collection BIGINT NOT NULL,
    checkpoint_operation_id VARCHAR(128) NOT NULL,
    collected_by_operation VARCHAR(128) NOT NULL,
    collected_at BIGINT NOT NULL,
    proof LONGTEXT NOT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
