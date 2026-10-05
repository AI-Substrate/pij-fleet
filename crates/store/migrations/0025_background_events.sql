-- Plan 163 P2 — event sources. A job of kind 'events' gets a per-job hook
-- (`pij bg emit` / POST /v1/bg/{job}/emit) authenticated by a secret whose
-- SHA-256 lives in token_hash; NULL means revoked (set at finish). Each emit is
-- one bg_events row; the daemon cuts pending rows into numbered batches and
-- delivers each batch once under msg_id `pij-bg:<job>:batch:<n>`.
-- `notified` remains the terminal-turn flag for every kind.
ALTER TABLE background_jobs ADD COLUMN kind TEXT NOT NULL DEFAULT 'oneshot'
    CHECK (kind IN ('oneshot', 'events'));
ALTER TABLE background_jobs ADD COLUMN token_hash TEXT
    CHECK (token_hash IS NULL OR length(token_hash) = 64);
ALTER TABLE background_jobs ADD COLUMN events_fyi INTEGER NOT NULL DEFAULT 0
    CHECK (events_fyi IN (0, 1));
ALTER TABLE background_jobs ADD COLUMN min_interval_ms INTEGER NOT NULL DEFAULT 60000
    CHECK (min_interval_ms >= 0);
ALTER TABLE background_jobs ADD COLUMN inline_max INTEGER NOT NULL DEFAULT 5
    CHECK (inline_max >= 0);
-- When the owner was last woken (or its batch routed/held), for --min-interval.
ALTER TABLE background_jobs ADD COLUMN last_wake_at INTEGER CHECK (last_wake_at >= 0);
-- Batches cut so far; the open batch, if any, is number `batches`.
ALTER TABLE background_jobs ADD COLUMN batches INTEGER NOT NULL DEFAULT 0 CHECK (batches >= 0);
-- Emits dropped over the pending cap since the last cut, and those carried by the open batch.
ALTER TABLE background_jobs ADD COLUMN dropped INTEGER NOT NULL DEFAULT 0 CHECK (dropped >= 0);
ALTER TABLE background_jobs ADD COLUMN open_dropped INTEGER NOT NULL DEFAULT 0
    CHECK (open_dropped >= 0);

CREATE TABLE bg_events (
    job_id    TEXT NOT NULL REFERENCES background_jobs (job_id),
    seq       INTEGER NOT NULL CHECK (seq > 0),
    ts        INTEGER NOT NULL CHECK (ts >= 0),
    text      TEXT NOT NULL,
    data_json TEXT,
    state     TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'delivered', 'held', 'routed')),
    batch_no  INTEGER CHECK (batch_no > 0),
    PRIMARY KEY (job_id, seq),
    CHECK (state = 'pending' OR batch_no IS NOT NULL)
) STRICT;

CREATE INDEX bg_events_pending ON bg_events (job_id, seq) WHERE state = 'pending';

PRAGMA user_version = 25;
