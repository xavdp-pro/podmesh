-- Publishers: what this host declared, every event it recorded, the durable transition a crash is
-- recovered from, and the takeover proof each epoch was verified under.
-- `at` is written between backticks here and read the same way by a portable statement.
CREATE TABLE IF NOT EXISTS publishers(
    resource VARCHAR(191) NOT NULL,
    hostname VARCHAR(255) NOT NULL,
    tunnel_uuid VARCHAR(64) NOT NULL,
    credential LONGTEXT NOT NULL,
    origin_port BIGINT NOT NULL,
    declared_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    PRIMARY KEY (resource)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS publisher_events(
    id BIGINT NOT NULL AUTO_INCREMENT,
    resource VARCHAR(191) NOT NULL,
    event VARCHAR(64) NOT NULL,
    `at` BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    detail LONGTEXT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS publisher_transitions(
    resource VARCHAR(191) NOT NULL,
    state VARCHAR(64) NOT NULL,
    epoch BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    changed_at BIGINT NOT NULL,
    PRIMARY KEY (resource)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS publisher_takeover_verified(
    resource VARCHAR(191) NOT NULL,
    epoch BIGINT NOT NULL,
    generation BIGINT NOT NULL,
    acquired_at BIGINT NOT NULL,
    boot_id VARCHAR(64) NOT NULL,
    authority_id VARCHAR(191) NOT NULL,
    authority_key VARCHAR(128) NOT NULL,
    proof LONGTEXT NOT NULL,
    verified LONGTEXT NOT NULL,
    verified_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    authority_quorum LONGTEXT NOT NULL DEFAULT '',
    authority_digest VARCHAR(64) NOT NULL DEFAULT '',
    PRIMARY KEY (resource, epoch)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
