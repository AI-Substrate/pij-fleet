-- 0012 — admit typed-to-pane delivery evidence.
--
-- SQLite cannot widen a CHECK constraint in place. Rebuild the bounded delivery
-- ledger, preserve every existing row and sequence, then restore its index. The
-- enum is intentionally closed over every DeliveryOrigin known at this schema:
-- typed-to-pane, injected-to-transport, verified-arrival, and reader-read.
ALTER TABLE delivered_messages RENAME TO delivered_messages_v11;

CREATE TABLE delivered_messages (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    recipient    TEXT NOT NULL,
    msg_id       TEXT NOT NULL,
    origin       TEXT NOT NULL CHECK (
        origin IN (
            'typed-to-pane',
            'injected-to-transport',
            'verified-arrival',
            'reader-read'
        )
    ),
    delivered_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (recipient, msg_id)
) STRICT;

INSERT INTO delivered_messages (seq, recipient, msg_id, origin, delivered_at)
SELECT seq, recipient, msg_id, origin, delivered_at
FROM delivered_messages_v11;

DROP TABLE delivered_messages_v11;

CREATE INDEX delivered_messages_recipient_order
    ON delivered_messages (recipient, seq DESC);
