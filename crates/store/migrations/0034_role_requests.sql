-- Role requests (plan 199, roadmap 001 s05, claimed by pm-001).
--
-- One row per `pij send role:<id>[/<instance>]`. `id` is the send's msg_id,
-- which the receipt prints as the request id (ruling C3) and s06's
-- `pij output --answers` names. A request with no live holder waits here as
-- `queued` until an assign drains it (ruling C6); one that reached a holder is
-- `delivered`, with that holder. `delivering` is the claim between the two:
-- a drain or a send takes it for one holder before dispatching, so no request
-- is ever dispatched to two holders (review of #129). `reply` and `limit_ms` are the requester's
-- reply mode (req-08) and time limit (req-22), read by s06 and s12.
CREATE TABLE role_requests (
    id           TEXT PRIMARY KEY,
    project      TEXT NOT NULL,
    role         TEXT NOT NULL,
    instance     TEXT NOT NULL DEFAULT '',
    requester    TEXT NOT NULL,
    body         TEXT NOT NULL,
    in_reply_to  TEXT,
    fyi          INTEGER NOT NULL DEFAULT 0 CHECK (fyi IN (0, 1)),
    reply        TEXT NOT NULL CHECK (reply IN ('wake', 'fyi', 'none')),
    limit_ms     INTEGER,
    state        TEXT NOT NULL CHECK (state IN ('queued', 'delivering', 'delivered')),
    holder       TEXT,
    created_at   INTEGER NOT NULL,
    delivered_at INTEGER
) STRICT;

CREATE INDEX role_requests_queued
    ON role_requests (project, role, created_at) WHERE state = 'queued';

PRAGMA user_version = 34;
