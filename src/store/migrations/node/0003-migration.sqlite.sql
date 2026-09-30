-- Live migration: the reservation a universe is held under, the transfer authorization issued for
-- it, the destination's restore claim, and the history and tombstones those leave behind.
-- Separate tables: earlier package versions keep working on this journal after a rollback, but
-- they do not enforce reservations, authorizations or restore claims.
CREATE TABLE IF NOT EXISTS migration_reservations(
    universe_uuid TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    image_id TEXT NOT NULL,
    source_host_uuid TEXT NOT NULL,
    destination_host_uuid TEXT NOT NULL,
    container_started_at TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    detail TEXT);

CREATE TABLE IF NOT EXISTS migration_authorizations(
    authorization_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    universe_uuid TEXT NOT NULL,
    checkpoint_operation_id TEXT NOT NULL,
    destination_host_uuid TEXT NOT NULL,
    handoff TEXT NOT NULL,
    handoff_sha256 TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    outcome TEXT,
    outcome_sha256 TEXT,
    completed_by_operation TEXT);

CREATE TABLE IF NOT EXISTS migration_restore_claims(
    authorization_id TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL,
    universe_uuid TEXT NOT NULL,
    handoff TEXT NOT NULL,
    handoff_sha256 TEXT NOT NULL,
    source_host_uuid TEXT NOT NULL,
    source_container_id TEXT NOT NULL,
    image_id TEXT NOT NULL,
    state TEXT NOT NULL,
    container_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    outcome TEXT,
    outcome_sha256 TEXT,
    detail TEXT);

CREATE TABLE IF NOT EXISTS migration_reservation_history(
    id INTEGER PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    image_id TEXT NOT NULL,
    source_host_uuid TEXT NOT NULL,
    destination_host_uuid TEXT NOT NULL,
    container_started_at TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    detail TEXT,
    archived_at INTEGER NOT NULL,
    archived_by_operation TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS migration_universe_tombstones(
    universe_uuid TEXT PRIMARY KEY,
    class TEXT NOT NULL,
    class_number INTEGER NOT NULL,
    container_id TEXT NOT NULL,
    container_absent_at_collection INTEGER NOT NULL,
    checkpoint_operation_id TEXT NOT NULL,
    collected_by_operation TEXT NOT NULL,
    collected_at INTEGER NOT NULL,
    proof TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS migration_collection_history(
    id INTEGER PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    class TEXT NOT NULL,
    class_number INTEGER NOT NULL,
    container_id TEXT NOT NULL,
    container_absent_at_collection INTEGER NOT NULL,
    checkpoint_operation_id TEXT NOT NULL,
    collected_by_operation TEXT NOT NULL,
    collected_at INTEGER NOT NULL,
    proof TEXT NOT NULL);
