-- The operation journal: one row per operation ID, and the attempts made under it.
-- operation_attempts is a separate table so that an experimental3 rollback, which inserts four
-- values into operations, keeps working on a journal written by this version.
CREATE TABLE IF NOT EXISTS operations(
    id VARCHAR(128) NOT NULL,
    request LONGTEXT NOT NULL,
    status VARCHAR(32) NOT NULL,
    result LONGTEXT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS operation_attempts(
    id BIGINT NOT NULL AUTO_INCREMENT,
    operation_id VARCHAR(128) NOT NULL,
    started_at BIGINT NOT NULL,
    finished_at BIGINT NULL,
    outcome LONGTEXT NULL,
    detail LONGTEXT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
