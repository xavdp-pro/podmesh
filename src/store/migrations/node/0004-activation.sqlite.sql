-- Activation: the policy a universe runs under, the lease that entitles this host to start it,
-- the epochs an external authority moved, and the history of both.
-- The columns later versions added by ALTER TABLE are part of the table here, in the order the
-- upgrades appended them, with the same defaults: a row written before a field existed reads as
-- no standby, no authority, epoch zero.
CREATE TABLE IF NOT EXISTS activation_policy(
    universe_uuid TEXT PRIMARY KEY,
    lease_seconds INTEGER NOT NULL,
    takeover_margin_seconds INTEGER NOT NULL,
    declared_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    desired_standbys INTEGER NOT NULL DEFAULT 0,
    eligible_hosts TEXT NOT NULL DEFAULT '[]',
    authorization_ref TEXT NOT NULL DEFAULT '',
    authority_id TEXT NOT NULL DEFAULT '',
    authority_key TEXT NOT NULL DEFAULT '',
    authority_quorum TEXT NOT NULL DEFAULT '',
    authority_serial INTEGER NOT NULL DEFAULT 0);

CREATE TABLE IF NOT EXISTS activation_leases(
    universe_uuid TEXT PRIMARY KEY,
    holder_host_uuid TEXT NOT NULL,
    generation INTEGER NOT NULL,
    acquired_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0,
    grant_id TEXT NOT NULL DEFAULT '');

CREATE TABLE IF NOT EXISTS activation_lease_history(
    id INTEGER PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    holder_host_uuid TEXT NOT NULL,
    generation INTEGER NOT NULL,
    event TEXT NOT NULL,
    at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    boot_id TEXT);

CREATE TABLE IF NOT EXISTS activation_epochs(
    universe_uuid TEXT PRIMARY KEY,
    authority_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    grant_id TEXT NOT NULL,
    replica_id TEXT NOT NULL,
    seen_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS activation_policy_changes(
    id INTEGER PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    from_digest TEXT NOT NULL,
    to_digest TEXT NOT NULL,
    how TEXT NOT NULL,
    certificate_digest TEXT NOT NULL,
    authorization_ref TEXT NOT NULL,
    at INTEGER NOT NULL,
    operation_id TEXT NOT NULL);
