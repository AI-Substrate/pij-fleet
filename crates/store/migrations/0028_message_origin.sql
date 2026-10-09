-- Plan 164 review F02: a message's identity is (origin machine, msg_id), each in
-- its own column, never one folded into the other. A paired machine's ids share
-- nothing with this daemon's: a forwarded `X` and a local `X` to the same seat
-- are two messages, and an exact retry from the same origin is still one.
-- '' is a local origin; an alias is never empty, so the two never meet.

-- Jobs: the live-row dedupe is (kind, origin, key).
ALTER TABLE jobs ADD COLUMN dedupe_origin TEXT NOT NULL DEFAULT '';
DROP INDEX IF EXISTS jobs_dedupe_live;
CREATE UNIQUE INDEX jobs_dedupe_live
    ON jobs (kind, dedupe_origin, dedupe_key)
    WHERE state IN ('pending', 'running');

-- The delivered ledger: unique per (recipient, sender machine, msg_id). SQLite
-- cannot change a UNIQUE constraint in place, so the bounded ledger is rebuilt
-- with every row and sequence preserved; every existing row is local.
ALTER TABLE delivered_messages RENAME TO delivered_messages_v27;

CREATE TABLE delivered_messages (
    seq            INTEGER PRIMARY KEY AUTOINCREMENT,
    recipient      TEXT NOT NULL,
    sender_machine TEXT NOT NULL DEFAULT '',
    msg_id         TEXT NOT NULL,
    origin         TEXT NOT NULL CHECK (
        origin IN (
            'typed-to-pane',
            'injected-to-transport',
            'verified-arrival',
            'reader-read'
        )
    ),
    delivered_at   INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (recipient, sender_machine, msg_id)
) STRICT;

INSERT INTO delivered_messages (seq, recipient, sender_machine, msg_id, origin, delivered_at)
SELECT seq, recipient, '', msg_id, origin, delivered_at
FROM delivered_messages_v27;

DROP TABLE delivered_messages_v27;

CREATE INDEX delivered_messages_recipient_order
    ON delivered_messages (recipient, seq DESC);

PRAGMA user_version = 28;
