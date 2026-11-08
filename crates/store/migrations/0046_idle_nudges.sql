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

-- Every own event of a seat, the ones that start or re-arm an idle stretch:
-- its reports, and `seat.activity` other than idle (a turn starting). Idle
-- activity is left out by the index itself, so the newest own event is one
-- reverse seek and "any own event after the anchor" one range seek, however
-- many idle records trail it (review of #189, P189-F001). The predicate is
-- repeated verbatim in crates/store/src/idle_nudges.rs (`OWN_EVENT`); the
-- queries name this index with INDEXED BY, so SQLite refuses them if the two
-- ever drift apart. `json_valid` keeps a malformed payload from failing the
-- spine write: such a row is simply not an own event.
CREATE INDEX spine_own_events ON spine_events (seat, seq, at)
    WHERE kind IN ('report.now', 'report.state')
       OR (kind = 'seat.activity'
           AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.state') END != 'idle');

-- A seat's live (queued or running) background jobs, by owner: one seek per
-- idle holder instead of reading every pending job (P189-F002).
CREATE INDEX background_jobs_live_by_owner ON background_jobs (owner)
    WHERE state IN ('queued', 'running');

PRAGMA user_version = 46;
