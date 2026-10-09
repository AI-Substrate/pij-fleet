-- Opt-in watchdogs (2026-10-09). PAs are watched by role and never appear
-- here; any other seat is watched only while it has a row. Any seat may turn
-- any seat's watchdog on or off, so the row records who did it.
CREATE TABLE seat_watchdogs (
    seat          TEXT PRIMARY KEY NOT NULL,
    interval_secs INTEGER NOT NULL CHECK(interval_secs > 0),
    set_by        TEXT NOT NULL,
    set_at        INTEGER NOT NULL
) STRICT;
