-- Community role holders (plan 195, roadmap 001 s04, claimed by pm-001).
--
-- One row per holding: a seat holding a role (and instance) in a project from
-- `from_at` until `to_at`. The governance branch's holders documents are the
-- truth; these rows index them and are rebuildable from the branch head.
-- `end_pending` marks an end recorded by `pij close` that the writer could not
-- commit yet; the next directory write for the project flushes it.
CREATE TABLE community_assignments (
    project     TEXT NOT NULL,
    role        TEXT NOT NULL,
    instance    TEXT NOT NULL DEFAULT '',
    seat        TEXT NOT NULL,
    harness     TEXT,
    session     TEXT,
    machine     TEXT,
    from_at     TEXT NOT NULL,
    to_at       TEXT,
    assigned_by TEXT NOT NULL,
    released_by TEXT,
    why         TEXT NOT NULL,
    release_why TEXT,
    end_pending INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, role, instance, seat, from_at)
) STRICT;

CREATE INDEX community_assignments_open_by_seat
    ON community_assignments (seat) WHERE to_at IS NULL;

PRAGMA user_version = 32;
