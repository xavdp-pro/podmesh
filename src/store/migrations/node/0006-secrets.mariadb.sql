-- Declared secrets: names, digests and sizes, never contents. `state` is the durable transition
-- (`declaring`, `effective`, `removing`) that reconciliation finishes or undoes after a crash.
CREATE TABLE IF NOT EXISTS secrets(
    name VARCHAR(191) NOT NULL,
    sha256 VARCHAR(64) NOT NULL,
    bytes BIGINT NOT NULL,
    declared_at BIGINT NOT NULL,
    operation_id VARCHAR(128) NOT NULL,
    authorization_ref LONGTEXT NOT NULL,
    removed_at BIGINT NULL,
    state VARCHAR(32) NOT NULL DEFAULT 'effective',
    PRIMARY KEY (name)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
