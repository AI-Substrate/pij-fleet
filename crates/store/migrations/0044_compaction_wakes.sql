-- Compaction wakes (plan 222, F111).
--
-- One row per compact control pij admits (a direct compact, or the compact a
-- memory turn held), keyed by the seat and the control's message id, written
-- BEFORE the compact is admitted (a failed write refuses it), so a replayed
-- admission records nothing new and no compact is admitted untracked. `settled` is NULL while the PA
-- watchdog round still owes it a decision; the round reads only those rows.
-- It is set BEFORE the resume message is sent ('woken'), so a restart or an
-- overlapping round never sends twice, or to say why no message was owed
-- (resumed, holds-nothing, gone, undelivered, superseded). 'woken' records
-- that the one wake was attempted, not that it was delivered: there is no
-- retry. The round supersedes an older pending row once a newer compact of
-- the same seat is delivered. Settled rows stay as history.
CREATE TABLE compaction_wakes (
    seat        TEXT NOT NULL,
    msg_id      TEXT NOT NULL,
    admitted_at INTEGER NOT NULL,
    settled     TEXT CHECK (settled IN
        ('woken', 'resumed', 'holds-nothing', 'gone', 'undelivered', 'superseded')),
    settled_at  INTEGER,
    PRIMARY KEY (seat, msg_id)
) STRICT;

CREATE INDEX compaction_wakes_pending ON compaction_wakes (seat, admitted_at)
    WHERE settled IS NULL;

PRAGMA user_version = 44;
