-- Modal-dialog stalls (plan 219, F47; number claimed with pm-001).
--
-- One row per seat: the stretch in which the PA watchdog last found it on its
-- harness's own dialog. The row is claimed BEFORE the parent and the PA are
-- told, keyed by the stretch's start (`quiet_since`, the seat's last own
-- event), so a restart or an overlapping round never tells twice; a later
-- stretch replaces it. `open` is whether the last pane read still showed the
-- dialog. A row whose seat has an own event after `quiet_since` is history:
-- `pij list` and the state card show it only while no such event exists.
CREATE TABLE modal_stalls (
    seat        TEXT PRIMARY KEY,
    quiet_since INTEGER NOT NULL,
    claimed_at  INTEGER NOT NULL,
    dialog      TEXT NOT NULL,
    key         TEXT NOT NULL,
    clears      TEXT NOT NULL,
    open        INTEGER NOT NULL CHECK (open IN (0, 1))
) STRICT;

PRAGMA user_version = 40;
