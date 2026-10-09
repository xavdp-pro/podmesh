-- Per-universe dedicated data volume declarations: capacity the operator recorded on a host whose
-- Podman storage can grow. Physical per-universe LVM is not implied; the row is the journal contract.
CREATE TABLE IF NOT EXISTS universe_volume_declarations(
    universe_uuid TEXT PRIMARY KEY,
    capacity_bytes INTEGER NOT NULL,
    declared_at INTEGER NOT NULL,
    declare_operation_id TEXT NOT NULL,
    declare_authorization_ref TEXT NOT NULL,
    last_grow_operation_id TEXT,
    last_grown_at INTEGER);
