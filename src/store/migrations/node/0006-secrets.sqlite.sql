-- Declared secrets: names, digests and sizes, never contents. `state` is the durable transition
-- (`declaring`, `effective`, `removing`) that reconciliation finishes or undoes after a crash.
CREATE TABLE IF NOT EXISTS secrets(
    name TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    declared_at INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    authorization_ref TEXT NOT NULL,
    removed_at INTEGER,
    state TEXT NOT NULL DEFAULT 'effective');
