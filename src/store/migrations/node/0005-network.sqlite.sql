-- The managed network: this host's declaration, the peer pools it routes to, the addresses it
-- allocated, the /32 routes it published, and the effect ledger that makes a crash recoverable.
-- Only a LIVE allocation is unique, per address and per universe; the released rows are history,
-- so a released address may be allocated again and a universe put back at its own address.
CREATE TABLE IF NOT EXISTS network_declaration(
    network_uuid TEXT PRIMARY KEY,
    prefix TEXT NOT NULL,
    pool TEXT NOT NULL,
    gateway TEXT NOT NULL,
    bridge TEXT NOT NULL,
    state TEXT NOT NULL,
    declared_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    authorization_ref TEXT NOT NULL,
    observed TEXT,
    nat_backend TEXT);

CREATE TABLE IF NOT EXISTS network_peer_pools(
    pool TEXT PRIMARY KEY,
    via TEXT NOT NULL,
    network_uuid TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS network_allocations(
    universe_uuid TEXT NOT NULL,
    network_uuid TEXT NOT NULL,
    ip TEXT NOT NULL,
    allocated_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    released_at INTEGER,
    released_by TEXT);

CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_ip ON network_allocations(ip) WHERE released_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS network_allocations_live_universe ON network_allocations(universe_uuid) WHERE released_at IS NULL;

CREATE TABLE IF NOT EXISTS network_routes(
    ip TEXT PRIMARY KEY,
    universe_uuid TEXT NOT NULL,
    via TEXT NOT NULL,
    published_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    exclusive_resource TEXT,
    alias_universe_uuid TEXT,
    state TEXT);

CREATE TABLE IF NOT EXISTS network_effects(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    owner TEXT NOT NULL,
    intent TEXT NOT NULL,
    state TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    changed_at INTEGER NOT NULL,
    observed TEXT);
