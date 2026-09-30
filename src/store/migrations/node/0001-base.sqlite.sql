-- The two tables every node open creates: what this journal is bound to, and what it observed.
-- `metadata` binds the journal to one machine identity; `observations` is the reply log.
CREATE TABLE IF NOT EXISTS metadata(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS observations(
    id INTEGER PRIMARY KEY,
    observed_at INTEGER NOT NULL,
    operation TEXT NOT NULL,
    result TEXT NOT NULL);
