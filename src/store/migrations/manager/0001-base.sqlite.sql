-- Manager replica history (schema v3 in legacy PRAGMA user_version terms).
CREATE TABLE IF NOT EXISTS identity (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    replica_id TEXT NOT NULL,
    topology_json TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS facts (
    event_id TEXT PRIMARY KEY,
    fact_json TEXT NOT NULL,
    sha256 TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS receipts (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    source_replica_id TEXT,
    wire_operation_id TEXT,
    request_json TEXT NOT NULL,
    response_json TEXT NOT NULL,
    sha256 TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS exchange_audit_events (
    audit_event_id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL,
    wire_nonce TEXT NOT NULL,
    direction TEXT NOT NULL,
    phase TEXT NOT NULL,
    authenticated_peer_id TEXT,
    peer_claim TEXT,
    operation_id TEXT,
    request_frame_bytes INTEGER NOT NULL,
    request_announced_body_bytes INTEGER,
    request_sha256 TEXT,
    reply_frame_bytes INTEGER NOT NULL,
    reply_announced_body_bytes INTEGER,
    reply_sha256 TEXT,
    outcome TEXT NOT NULL,
    error_category TEXT,
    reason_code TEXT,
    local_receipt_operation_id TEXT,
    local_receipt_sha256 TEXT,
    remote_receipt_operation_id TEXT,
    remote_receipt_sha256 TEXT,
    replayed INTEGER NOT NULL CHECK(replayed IN (0, 1)),
    record_json TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    UNIQUE(direction, attempt_id, phase));

CREATE TRIGGER IF NOT EXISTS facts_no_update BEFORE UPDATE ON facts
BEGIN SELECT RAISE(ABORT, 'immutable fact'); END;
CREATE TRIGGER IF NOT EXISTS facts_no_delete BEFORE DELETE ON facts
BEGIN SELECT RAISE(ABORT, 'immutable fact'); END;
CREATE TRIGGER IF NOT EXISTS identity_no_update BEFORE UPDATE ON identity
BEGIN SELECT RAISE(ABORT, 'immutable identity'); END;
CREATE TRIGGER IF NOT EXISTS identity_no_delete BEFORE DELETE ON identity
BEGIN SELECT RAISE(ABORT, 'immutable identity'); END;
CREATE TRIGGER IF NOT EXISTS receipts_no_update BEFORE UPDATE ON receipts
BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;
CREATE TRIGGER IF NOT EXISTS receipts_no_delete BEFORE DELETE ON receipts
BEGIN SELECT RAISE(ABORT, 'immutable receipt'); END;
CREATE TRIGGER IF NOT EXISTS exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events
BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;
CREATE TRIGGER IF NOT EXISTS exchange_audit_events_no_delete BEFORE DELETE ON exchange_audit_events
BEGIN SELECT RAISE(ABORT, 'immutable exchange audit event'); END;
