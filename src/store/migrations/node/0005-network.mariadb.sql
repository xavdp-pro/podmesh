-- The managed network: this host's declaration, the peer pools it routes to, the addresses it
-- allocated, the /32 routes it published, and the effect ledger that makes a crash recoverable.
--
-- The one construct of this whole set that has no equivalent on this engine: SQLite enforces
-- "unique among live allocations" with a partial index (`... WHERE released_at IS NULL`), and
-- MariaDB has no partial index. The same rule is carried by a virtual column that is the address
-- while the allocation is live and NULL once it is released, under a plain unique key: a unique
-- key ignores NULLs on both engines, so a released row stops taking part in uniqueness exactly as
-- it does under the partial index. The virtual columns are never written or read by a caller.
CREATE TABLE IF NOT EXISTS network_declaration(
    network_uuid VARCHAR(64) NOT NULL,
    prefix VARCHAR(64) NOT NULL,
    pool VARCHAR(64) NOT NULL,
    gateway VARCHAR(45) NOT NULL,
    bridge VARCHAR(64) NOT NULL,
    state VARCHAR(64) NOT NULL,
    declared_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    observed LONGTEXT NULL,
    nat_backend VARCHAR(64) NULL,
    PRIMARY KEY (network_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS network_peer_pools(
    pool VARCHAR(64) NOT NULL,
    via VARCHAR(45) NOT NULL,
    network_uuid VARCHAR(64) NOT NULL,
    PRIMARY KEY (pool)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS network_allocations(
    universe_uuid VARCHAR(64) NOT NULL,
    network_uuid VARCHAR(64) NOT NULL,
    ip VARCHAR(45) NOT NULL,
    allocated_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    released_at BIGINT NULL,
    released_by VARCHAR(128) NULL,
    live_ip VARCHAR(45) AS (IF(released_at IS NULL, ip, NULL)) VIRTUAL,
    live_universe_uuid VARCHAR(64) AS (IF(released_at IS NULL, universe_uuid, NULL)) VIRTUAL,
    UNIQUE KEY network_allocations_live_ip (live_ip),
    UNIQUE KEY network_allocations_live_universe (live_universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS network_routes(
    ip VARCHAR(45) NOT NULL,
    universe_uuid VARCHAR(64) NOT NULL,
    via VARCHAR(45) NOT NULL,
    published_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    exclusive_resource VARCHAR(191) NULL,
    alias_universe_uuid VARCHAR(64) NULL,
    state VARCHAR(64) NULL,
    PRIMARY KEY (ip)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS network_effects(
    id BIGINT NOT NULL AUTO_INCREMENT,
    kind VARCHAR(64) NOT NULL,
    `key` VARCHAR(191) NOT NULL,
    owner VARCHAR(191) NOT NULL,
    intent VARCHAR(64) NOT NULL,
    state VARCHAR(64) NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    changed_at BIGINT NOT NULL,
    observed LONGTEXT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
