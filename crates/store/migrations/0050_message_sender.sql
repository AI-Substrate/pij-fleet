-- A message's identity includes its sender seat (F270).
--
-- Until now a message was (recipient, sender machine, msg_id): a seat that
-- reused another seat's msg_id collapsed into that seat's message. Its own
-- body was never delivered, and since F260 an expiry notice could also be
-- silenced. msg_ids are chosen by the sender, so they are only unique per
-- sender. A message is now (recipient, sender machine, sender seat, msg_id):
-- the same id from two seats is two messages, and both are delivered. An
-- exact retry from the same seat is still one.

-- Jobs: the sender seat is the delivery payload's `from`, derived and never
-- stored (VIRTUAL), so no write path changes. Every other job kind has ''.
ALTER TABLE jobs ADD COLUMN dedupe_sender TEXT GENERATED ALWAYS AS (
    CASE WHEN (substr(kind, 1, 9) = 'delivery:' OR substr(kind, 1, 17) = 'delivery-control:')
              AND json_valid(payload)
         THEN coalesce(json_extract(payload, '$.from'), '')
         ELSE '' END
) VIRTUAL;

-- The sender is the last column of each key, so a lookup that has no sender
-- still seeks on the old prefix.
DROP INDEX IF EXISTS jobs_dedupe_live;
CREATE UNIQUE INDEX jobs_dedupe_live
    ON jobs (kind, dedupe_origin, dedupe_key, dedupe_sender)
    WHERE state IN ('pending', 'running');

DROP INDEX IF EXISTS jobs_by_dedupe;
CREATE INDEX jobs_by_dedupe
    ON jobs (kind, dedupe_origin, dedupe_key, dedupe_sender);

-- F270 (b): native holds (F260), so an expiry seeks only a message's
-- unexpired holds (`not_before` is the deadline), not its failed history.
CREATE INDEX jobs_native_held
    ON jobs (kind, dedupe_origin, dedupe_key, dedupe_sender, not_before)
    WHERE state = 'failed' AND outcome = 'undelivered:native-receiver-unavailable';

-- The delivered ledger: unique per (recipient, sender machine, msg_id, sender
-- seat). SQLite cannot change a UNIQUE constraint in place, so the bounded
-- ledger is rebuilt with every row and sequence preserved. No existing row
-- records its sender, so each keeps sender_seat ''. A lookup treats '' as any
-- sender, so a retry across the upgrade still finds its delivery (a brake:
-- removing it only delivers more). Every new row names its sender.
ALTER TABLE delivered_messages RENAME TO delivered_messages_v49;

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
    sender_seat    TEXT NOT NULL DEFAULT '',
    UNIQUE (recipient, sender_machine, msg_id, sender_seat)
) STRICT;

INSERT INTO delivered_messages (seq, recipient, sender_machine, msg_id, origin, delivered_at, sender_seat)
SELECT seq, recipient, sender_machine, msg_id, origin, delivered_at, ''
FROM delivered_messages_v49;

DROP TABLE delivered_messages_v49;

CREATE INDEX delivered_messages_recipient_order
    ON delivered_messages (recipient, seq DESC);

PRAGMA user_version = 50;
