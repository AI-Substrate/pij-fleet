-- Idle-holder nudges (plan 224, F136).
--
-- One row per idle stretch a seat holding work was nudged for, keyed by the
-- seat and the spine sequence of its last own event (`anchor`): the stretch
-- that event began. The PA watchdog round inserts the row BEFORE it sends the
-- nudge, and sends only if the insert added it, so a restart or an
-- overlapping round never nudges twice for one stretch. The seat's next own
-- event starts a new stretch with a new anchor. Rows stay as history.
CREATE TABLE idle_nudges (
    seat      TEXT NOT NULL,
    anchor    INTEGER NOT NULL,
    nudged_at INTEGER NOT NULL,
    PRIMARY KEY (seat, anchor)
) STRICT;

PRAGMA user_version = 46;
