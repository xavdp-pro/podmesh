-- Activation: the policy a universe runs under, the lease that entitles this host to start it,
-- the epochs an external authority moved, and the history of both.
-- `at` is written between backticks here and read the same way by a portable statement.
-- A TEXT column carries a DEFAULT on this engine from 10.2.1 onward, which is what the SQLite
-- side declares; the target of this migration is MariaDB 11.
CREATE TABLE IF NOT EXISTS activation_policy(
    universe_uuid VARCHAR(64) NOT NULL,
    lease_seconds BIGINT NOT NULL,
    takeover_margin_seconds BIGINT NOT NULL,
    declared_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    desired_standbys BIGINT NOT NULL DEFAULT 0,
    eligible_hosts LONGTEXT NOT NULL DEFAULT '[]',
    authorization_ref LONGTEXT NOT NULL DEFAULT '',
    authority_id VARCHAR(191) NOT NULL DEFAULT '',
    authority_key VARCHAR(128) NOT NULL DEFAULT '',
    authority_quorum LONGTEXT NOT NULL DEFAULT '',
    authority_serial BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS activation_leases(
    universe_uuid VARCHAR(64) NOT NULL,
    holder_host_uuid VARCHAR(64) NOT NULL,
    generation BIGINT NOT NULL,
    acquired_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    epoch BIGINT NOT NULL DEFAULT 0,
    grant_id VARCHAR(191) NOT NULL DEFAULT '',
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS activation_lease_history(
    id BIGINT NOT NULL AUTO_INCREMENT,
    universe_uuid VARCHAR(64) NOT NULL,
    holder_host_uuid VARCHAR(64) NOT NULL,
    generation BIGINT NOT NULL,
    event VARCHAR(64) NOT NULL,
    `at` BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    boot_id VARCHAR(64) NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS activation_epochs(
    universe_uuid VARCHAR(64) NOT NULL,
    authority_id VARCHAR(191) NOT NULL,
    epoch BIGINT NOT NULL,
    grant_id VARCHAR(191) NOT NULL,
    replica_id VARCHAR(191) NOT NULL,
    seen_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS activation_policy_changes(
    id BIGINT NOT NULL AUTO_INCREMENT,
    universe_uuid VARCHAR(64) NOT NULL,
    from_digest VARCHAR(64) NOT NULL,
    to_digest VARCHAR(64) NOT NULL,
    how VARCHAR(64) NOT NULL,
    certificate_digest VARCHAR(64) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    `at` BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
