-- Publishers: what this host declared, every event it recorded, the durable transition a crash is
-- recovered from, and the takeover proof each epoch was verified under.
CREATE TABLE IF NOT EXISTS publishers(
    resource TEXT PRIMARY KEY,
    hostname TEXT NOT NULL,
    tunnel_uuid TEXT NOT NULL,
    credential TEXT NOT NULL,
    origin_port INTEGER NOT NULL,
    declared_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    authorization_ref TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS publisher_events(
    id INTEGER PRIMARY KEY,
    resource TEXT NOT NULL,
    event TEXT NOT NULL,
    at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    detail TEXT);

CREATE TABLE IF NOT EXISTS publisher_transitions(
    resource TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    changed_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS publisher_takeover_verified(
    resource TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL,
    boot_id TEXT NOT NULL,
    authority_id TEXT NOT NULL,
    authority_key TEXT NOT NULL,
    proof TEXT NOT NULL,
    verified TEXT NOT NULL,
    verified_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    authority_quorum TEXT NOT NULL DEFAULT '',
    authority_digest TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(resource, epoch));
