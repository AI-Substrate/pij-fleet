-- PA presence and large seats (plan 217, 2026-11-07).
--
-- presence_claims: one row per check and subject while its message stands for
-- the current stretch. The row is claimed before the message is sent, so a
-- daemon restart or an overlapping round never sends it twice; deleting it
-- re-arms the check. `kind` is pa-missing (subject: the scope's repository
-- key), pa-dormant and pa-unresponsive (subject: the PA), or large-seat
-- (subject: the seat). `detail` is the delivery's answer, as JSON, once known.
CREATE TABLE presence_claims (
    kind       TEXT NOT NULL,
    subject    TEXT NOT NULL,
    claimed_at INTEGER NOT NULL,
    detail     TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (kind, subject)
) STRICT;

PRAGMA user_version = 38;
