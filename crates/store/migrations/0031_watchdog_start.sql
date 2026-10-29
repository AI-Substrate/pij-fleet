-- Opt-in watchdog start interval (plan 184): `pij watchdog on <seat> --start 2m --every 20m`.
-- start_secs is the first check's quiet interval (NULL: every check uses interval_secs).
-- started_at is when the start phase ended, in epoch ms: the seat's first report.now after
-- arming or the start nudge, whichever came first (NULL while it runs). Rows from before
-- this migration read as --every alone.
ALTER TABLE seat_watchdogs ADD COLUMN start_secs INTEGER CHECK(start_secs IS NULL OR start_secs > 0);
ALTER TABLE seat_watchdogs ADD COLUMN started_at INTEGER;

PRAGMA user_version = 31;
