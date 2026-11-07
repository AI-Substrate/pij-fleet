-- Held deliveries and mid-turn steering (plan 214, AC2 and AC3, ruling C31;
-- number from pij claim).
--
-- A delivery's held time is `jobs.acked_at - jobs.enqueued_at`: queued to
-- reached-the-session on every rung (the ack is written after the body reaches
-- the recipient). Those columns stay the record; this table only remembers
-- which long holds the PA digest has shown, so each is shown once.
--
-- A row is written by the ack that ends a hold longer than 15 minutes (the
-- threshold is `HELD_FLAG_SECS` in the store), or by the digest that shows a
-- message still queued past it. `flagged_at` is set when a digest shows it.
-- A PA's digest reads only its own fleet's unflagged rows, by recipient,
-- through the partial index, so another fleet's holds never fill its page;
-- the still-queued half reads each recipient's live jobs through jobs_serial.
CREATE TABLE held_deliveries (
    job_id     INTEGER PRIMARY KEY,
    recipient  TEXT NOT NULL,
    flagged_at INTEGER
) STRICT;

CREATE INDEX held_deliveries_unflagged
    ON held_deliveries (recipient, job_id) WHERE flagged_at IS NULL;

-- Mid-turn steering, per seat (C31). A native Copilot seat's extension keeps
-- claiming while a turn it started is still running and steers each normal
-- send into it. No row means on; `pij steer off <seat>` writes `steer = 0`, and
-- the claim made during such a turn reads it, so the switch takes effect
-- without a restart and is never polled.
CREATE TABLE seat_steering (
    seat      TEXT PRIMARY KEY,
    steer     INTEGER NOT NULL CHECK (steer IN (0, 1)),
    set_by    TEXT NOT NULL,
    set_at_ms INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 43;
