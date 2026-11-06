-- Number claims and deliveries (plan 196, roadmap 001 s11, claimed by pm-001).
--
-- claims: one row per number handed out for (project, kind). A released row is
-- kept as history and its number is never handed out again. `seat` is the
-- claiming seat; tombstoning it releases its open claims (reason seat-closed).
CREATE TABLE claims (
    project        TEXT NOT NULL,
    kind           TEXT NOT NULL,
    number         INTEGER NOT NULL,
    seat           TEXT NOT NULL,
    subject        TEXT NOT NULL,
    claimed_at     INTEGER NOT NULL,
    released_at    INTEGER,
    released_by    TEXT,
    release_reason TEXT,
    PRIMARY KEY (project, kind, number)
) STRICT;

CREATE INDEX claims_open_by_seat ON claims (seat) WHERE released_at IS NULL;

-- deliveries: what a merged PR delivered, one row per ddocs address, recorded
-- once by `pij delivered` (ruling C23). `pm` is the seat told on its card.
CREATE TABLE deliveries (
    project      TEXT NOT NULL,
    pr           INTEGER NOT NULL,
    address      TEXT NOT NULL,
    merge_commit TEXT NOT NULL,
    recorded_by  TEXT NOT NULL,
    recorded_at  INTEGER NOT NULL,
    pm           TEXT,
    PRIMARY KEY (project, pr, address)
) STRICT;

CREATE INDEX deliveries_by_pm ON deliveries (pm, recorded_at) WHERE pm IS NOT NULL;

-- Every PR recorded for one address, so a printed delivered_by edit carries
-- all of them (review of #131, F002).
CREATE INDEX deliveries_by_address ON deliveries (project, address, recorded_at);

PRAGMA user_version = 33;
