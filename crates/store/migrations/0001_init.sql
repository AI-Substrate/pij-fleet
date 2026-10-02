-- 0001 — the mold's first schema.
--
-- Two facts are set here on purpose:
--
--   * `user_version` is the SCHEMA version this binary reasons about. sqlx's own
--     `_sqlx_migrations` table records which migrations ran and their checksums;
--     `user_version` is the one-integer answer a cheap boot check can read
--     without joining anything, which is what makes "refuse loudly on skew"
--     affordable on every db-touching command.
--   * WAL is set on the CONNECTION rather than here: `PRAGMA journal_mode` is a
--     no-op inside a transaction, and sqlx runs each migration in one. Setting it
--     in this file would appear to work and silently leave the database in
--     rollback-journal mode — the failure this comment exists to prevent.

CREATE TABLE seats (
    id              TEXT PRIMARY KEY NOT NULL,
    harness         TEXT NOT NULL,
    pane            TEXT,
    -- Liveness identity is the PAIR. A pid alone is not an identity: the OS
    -- recycles it, and the recycled case must be distinguishable from both alive
    -- and dead.
    pid             INTEGER,
    proc_start      INTEGER,
    folder          TEXT NOT NULL,
    state           TEXT NOT NULL,
    semantic_state  TEXT,
    role            TEXT,
    parent          TEXT,
    relay           INTEGER NOT NULL DEFAULT 0,
    -- A tombstone keeps the row: a seat that vanishes takes its own post-mortem
    -- with it, so death is a column, never a DELETE.
    tombstoned_at   INTEGER,
    tombstone_reason TEXT,
    seq             INTEGER NOT NULL
) STRICT;

CREATE INDEX seats_by_folder ON seats (folder);
CREATE INDEX seats_by_parent ON seats (parent);

-- The append-only history. Rows are never updated; `seq` is the total order
-- every tail and every anomaly detector reads.
CREATE TABLE spine_events (
    seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    v        INTEGER NOT NULL,
    at       INTEGER NOT NULL,
    kind     TEXT NOT NULL,
    seat     TEXT,
    payload  TEXT NOT NULL
) STRICT;

CREATE INDEX spine_by_seat ON spine_events (seat, seq);

PRAGMA user_version = 1;
