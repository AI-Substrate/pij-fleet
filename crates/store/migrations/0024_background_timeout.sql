-- Plan 163 P1 — an optional run-time limit per background job. The deadline is
-- absolute so it survives daemon restarts; `timed_out` records that the daemon,
-- not a caller, requested the kill, so the completion turn can say TIMEOUT.
ALTER TABLE background_jobs ADD COLUMN deadline_at INTEGER CHECK (deadline_at >= 0);
ALTER TABLE background_jobs ADD COLUMN timed_out INTEGER NOT NULL DEFAULT 0
    CHECK (timed_out IN (0, 1) AND (timed_out = 0 OR kill_requested = 1));

PRAGMA user_version = 24;
