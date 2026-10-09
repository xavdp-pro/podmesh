-- The two tables every node open creates: what this journal is bound to, and what it observed.
-- `key` is a reserved word on this engine and is written between backticks, which SQLite accepts
-- too; the node's own statements quote it the same way so one statement reads on both engines.
-- `id INTEGER PRIMARY KEY` is SQLite's rowid alias, which is AUTO_INCREMENT here.
CREATE TABLE IF NOT EXISTS metadata(
    `key` VARCHAR(191) NOT NULL,
    value LONGTEXT NOT NULL,
    PRIMARY KEY (`key`)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS observations(
    id BIGINT NOT NULL AUTO_INCREMENT,
    observed_at BIGINT NOT NULL,
    operation VARCHAR(64) NOT NULL,
    result LONGTEXT NOT NULL,
    PRIMARY KEY (id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
