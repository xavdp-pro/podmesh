-- Manager replica history (schema v3 in legacy SQLite PRAGMA user_version terms).
CREATE TABLE IF NOT EXISTS identity (
    singleton INT NOT NULL,
    replica_id VARCHAR(128) NOT NULL,
    topology_json LONGTEXT NOT NULL,
    PRIMARY KEY (singleton),
    CONSTRAINT identity_singleton CHECK (singleton = 1)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS facts (
    event_id VARCHAR(191) NOT NULL,
    fact_json LONGTEXT NOT NULL,
    sha256 CHAR(64) NOT NULL,
    PRIMARY KEY (event_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS receipts (
    operation_id VARCHAR(191) NOT NULL,
    kind VARCHAR(64) NOT NULL,
    source_replica_id VARCHAR(128),
    wire_operation_id VARCHAR(191),
    request_json LONGTEXT NOT NULL,
    response_json LONGTEXT NOT NULL,
    sha256 CHAR(64) NOT NULL,
    PRIMARY KEY (operation_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE IF NOT EXISTS exchange_audit_events (
    audit_event_id VARCHAR(191) NOT NULL,
    attempt_id VARCHAR(256) NOT NULL,
    wire_nonce VARCHAR(256) NOT NULL,
    direction VARCHAR(32) NOT NULL,
    phase VARCHAR(32) NOT NULL,
    authenticated_peer_id VARCHAR(128),
    peer_claim LONGTEXT,
    operation_id VARCHAR(191),
    request_frame_bytes BIGINT NOT NULL,
    request_announced_body_bytes BIGINT,
    request_sha256 CHAR(64),
    reply_frame_bytes BIGINT NOT NULL,
    reply_announced_body_bytes BIGINT,
    reply_sha256 CHAR(64),
    outcome VARCHAR(64) NOT NULL,
    error_category VARCHAR(64),
    reason_code VARCHAR(128),
    local_receipt_operation_id VARCHAR(191),
    local_receipt_sha256 CHAR(64),
    remote_receipt_operation_id VARCHAR(191),
    remote_receipt_sha256 CHAR(64),
    replayed TINYINT NOT NULL,
    record_json LONGTEXT NOT NULL,
    sha256 CHAR(64) NOT NULL,
    PRIMARY KEY (audit_event_id),
    CONSTRAINT exchange_audit_replayed CHECK (replayed IN (0, 1)),
    UNIQUE KEY exchange_audit_direction_attempt_phase (direction, attempt_id, phase)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

-- Single-statement triggers: MariaDB batches split on ';' and must not use BEGIN…END here.
DROP TRIGGER IF EXISTS facts_no_update;
CREATE TRIGGER facts_no_update BEFORE UPDATE ON facts FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable fact';

DROP TRIGGER IF EXISTS facts_no_delete;
CREATE TRIGGER facts_no_delete BEFORE DELETE ON facts FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable fact';

DROP TRIGGER IF EXISTS identity_no_update;
CREATE TRIGGER identity_no_update BEFORE UPDATE ON identity FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable identity';

DROP TRIGGER IF EXISTS identity_no_delete;
CREATE TRIGGER identity_no_delete BEFORE DELETE ON identity FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable identity';

DROP TRIGGER IF EXISTS receipts_no_update;
CREATE TRIGGER receipts_no_update BEFORE UPDATE ON receipts FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable receipt';

DROP TRIGGER IF EXISTS receipts_no_delete;
CREATE TRIGGER receipts_no_delete BEFORE DELETE ON receipts FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable receipt';

DROP TRIGGER IF EXISTS exchange_audit_events_no_update;
CREATE TRIGGER exchange_audit_events_no_update BEFORE UPDATE ON exchange_audit_events FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable exchange audit event';

DROP TRIGGER IF EXISTS exchange_audit_events_no_delete;
CREATE TRIGGER exchange_audit_events_no_delete BEFORE DELETE ON exchange_audit_events FOR EACH ROW
    SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'immutable exchange audit event';
