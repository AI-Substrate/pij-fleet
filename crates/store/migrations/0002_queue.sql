-- 0002 — the job queue (workshop 001 R4).
--
-- Built BEFORE any consumer, deliberately: lynx's crew re-derived queue
-- semantics mid-flight twice, and each re-derivation was a different queue.
--
-- Two invariants live in this file rather than in code, because an invariant a
-- process enforces is an invariant that stops being true the moment a second
-- process exists:
--
--   * DEDUPE is a PARTIAL unique index over LIVE rows only. N rapid submits of
--     the same work collapse to one row; once that row is acked the key is free
--     again, so "one row per burst" never becomes "one row for ever".
--   * SERIALIZATION is per `serial_key`: at most one RUNNING job per entity.
--     Enforced by the claim query, and asserted by a contention test with
--     parallel claimers.

CREATE TABLE jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL,
    serial_key  TEXT NOT NULL,
    payload     TEXT NOT NULL,
    dedupe_key  TEXT NOT NULL,
    -- pending -> running -> done|failed. A job never returns to pending: a retry
    -- is a NEW row, so the history of what was attempted survives.
    state       TEXT NOT NULL DEFAULT 'pending',
    worker      TEXT,
    outcome     TEXT,
    enqueued_at INTEGER NOT NULL DEFAULT 0,
    claimed_at  INTEGER,
    acked_at    INTEGER
) STRICT;

-- The dedupe rule, as a constraint rather than a convention: a second live row
-- with the same key cannot be inserted even by a different process.
CREATE UNIQUE INDEX jobs_dedupe_live
    ON jobs (dedupe_key)
    WHERE state IN ('pending', 'running');

CREATE INDEX jobs_claimable ON jobs (state, kind, id);
CREATE INDEX jobs_serial ON jobs (serial_key, state);

PRAGMA user_version = 2;
