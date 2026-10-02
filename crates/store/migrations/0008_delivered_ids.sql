-- 0008 — bounded destination-side delivered-id ledger.
--
-- A live queue row collapses duplicate forwards only until the recipient claims
-- and acknowledges it. After that, the live dedupe key is free, so an ambiguous
-- federation retry needs durable evidence from the DESTINATION that the message
-- was delivered. The origin travels with that evidence so a suppressed retry can
-- replay an honest receipt instead of claiming a queue row that does not exist.
--
-- Retention is enforced by SqliteQueue::ack_delivery in the same write
-- transaction as the acknowledgement. The configured bound is per recipient: a
-- hot recipient cannot erase every other recipient's duplicate protection.
CREATE TABLE delivered_messages (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    recipient    TEXT NOT NULL,
    msg_id       TEXT NOT NULL,
    origin       TEXT NOT NULL CHECK (
        origin IN ('injected-to-transport', 'verified-arrival', 'reader-read')
    ),
    delivered_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (recipient, msg_id)
) STRICT;

CREATE INDEX delivered_messages_recipient_order
    ON delivered_messages (recipient, seq DESC);
