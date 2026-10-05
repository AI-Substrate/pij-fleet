-- Plan 163 P1 — an optional run-time limit per background job. The deadline is
-- absolute so it survives daemon restarts; `timed_out` records that the daemon,
-- not a caller, requested the kill, so the completion turn can say TIMEOUT.
-- `term_sent` is persisted immediately before the daemon sends TERM: the only
-- provenance that a timeout (not the command itself) ended the runner.
ALTER TABLE background_jobs ADD COLUMN deadline_at INTEGER CHECK (deadline_at >= 0);
ALTER TABLE background_jobs ADD COLUMN timed_out INTEGER NOT NULL DEFAULT 0
    CHECK (timed_out IN (0, 1) AND (timed_out = 0 OR kill_requested = 1));
ALTER TABLE background_jobs ADD COLUMN term_sent INTEGER NOT NULL DEFAULT 0
    CHECK (term_sent IN (0, 1) AND (term_sent = 0 OR kill_requested = 1));

PRAGMA user_version = 24;
