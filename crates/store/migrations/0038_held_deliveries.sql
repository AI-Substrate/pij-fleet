-- Deliveries held behind a busy turn (plan 214, AC2; number from pij claim).
--
-- A delivery's held time is `jobs.acked_at - jobs.enqueued_at`: queued to
-- reached-the-session on every rung (the ack is written after the body reaches
-- the recipient). Those columns stay the record; this table only remembers
-- which long holds the PA digest has shown, so each is shown once.
--
-- A row is written by the ack that ends a hold longer than 15 minutes (the
-- threshold is `HELD_FLAG_SECS` in the store), or by the digest that shows a
-- message still queued past it. `flagged_at` is set when a digest shows it.
-- The digest reads only unflagged rows, through the partial index.
CREATE TABLE held_deliveries (
    job_id     INTEGER PRIMARY KEY,
    flagged_at INTEGER
) STRICT;

CREATE INDEX held_deliveries_unflagged
    ON held_deliveries (job_id) WHERE flagged_at IS NULL;

-- Still-queued deliveries, oldest first, for the digest's live half.
CREATE INDEX jobs_live_delivery
    ON jobs (enqueued_at) WHERE state IN ('pending', 'running') AND kind LIKE 'delivery:%';

PRAGMA user_version = 38;
