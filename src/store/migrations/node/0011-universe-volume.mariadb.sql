-- Per-universe dedicated data volume declarations: capacity the operator recorded on a host whose
-- Podman storage can grow. Physical per-universe LVM is not implied; the row is the journal contract.
CREATE TABLE IF NOT EXISTS universe_volume_declarations(
    universe_uuid VARCHAR(64) NOT NULL,
    capacity_bytes BIGINT NOT NULL,
    declared_at BIGINT NOT NULL,
    declare_operation_id VARCHAR(128) NOT NULL,
    declare_authorization_ref LONGTEXT NOT NULL,
    last_grow_operation_id VARCHAR(128),
    last_grown_at BIGINT,
    PRIMARY KEY (universe_uuid)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
