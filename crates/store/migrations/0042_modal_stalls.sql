-- Modal-dialog stalls (plan 219, F47; 0042: 0040 went to #149, 0041 is reserved for #152).
--
-- One row per seat: the stretch in which the PA watchdog last found it on its
-- harness's own dialog. The row is claimed BEFORE the parent and the PA are
-- told, keyed by the stretch's identity (`quiet_seq`, the spine sequence of the
-- seat's last own event; `quiet_since` is that event's time, for duration
-- only), so a restart or an overlapping round never tells twice; a later
-- stretch (a higher sequence) replaces it. `told` lists the recipients that
-- accepted the message; `attempts` counts each recipient's tells, saved BEFORE
-- each is sent, so a recipient still owed is tried on later rounds and never
-- more than the bound, even when a save fails (nothing is then sent). `dialog` is the
-- marker's fixed label, never pane text. `open` is whether the last pane read
-- still showed the dialog. A row whose seat has an own event after `quiet_seq`
-- is history: `pij list` and the state card show it only while none exists.
CREATE TABLE modal_stalls (
    seat        TEXT PRIMARY KEY,
    quiet_seq   INTEGER NOT NULL,
    quiet_since INTEGER NOT NULL,
    claimed_at  INTEGER NOT NULL,
    dialog      TEXT NOT NULL,
    key         TEXT NOT NULL,
    clears      TEXT NOT NULL,
    open        INTEGER NOT NULL CHECK (open IN (0, 1)),
    told        TEXT NOT NULL DEFAULT '[]',
    attempts    TEXT NOT NULL DEFAULT '{}'
) STRICT;

PRAGMA user_version = 42;
