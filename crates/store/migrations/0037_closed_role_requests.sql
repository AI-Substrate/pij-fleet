-- Closed role requests (plan 212, s05 dogfood F59 and F61; number from pij claim).
--
-- A queued request that no holder will ever take is closed: withdrawn by its
-- requester, the PA or the prime (`pij requests withdraw <id> --why`), or
-- closed by the drain when the seat assigned the role is the request's own
-- requester ("the requester took the role"). Closing moves the row here and
-- deletes it from `role_requests` in one transaction, so the queue, the drain
-- and the PA digest never see it again, and `role_requests.state` keeps its
-- three values (no table rebuild). The columns are the request as sent, plus
-- who closed it, when and why. A delivered request that was answered or
-- abandoned closes the same way (`pij requests close <id>`, F83), which stops
-- its time limit; `holder` and `delivered_at` keep who had it and when.
CREATE TABLE closed_role_requests (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL,
    role        TEXT NOT NULL,
    instance    TEXT NOT NULL DEFAULT '',
    requester   TEXT NOT NULL,
    body        TEXT NOT NULL,
    in_reply_to TEXT,
    fyi         INTEGER NOT NULL DEFAULT 0 CHECK (fyi IN (0, 1)),
    reply       TEXT NOT NULL CHECK (reply IN ('wake', 'fyi', 'none')),
    limit_ms    INTEGER,
    created_at  INTEGER NOT NULL,
    holder      TEXT,
    delivered_at INTEGER,
    -- What a delivered request's close owes its requester: nothing asked
    -- (`quiet`), an FYI not yet admitted (`owed`), or sent (`told`). NULL for a
    -- withdraw or drain close. A close that ends with `owed` is retried by the
    -- next `pij requests close <id>` (review of #149, F005).
    notice      TEXT CHECK (notice IN ('quiet', 'owed', 'told')),
    -- How the closer was bound when it closed (`pane (derived)` or an
    -- asserted id), recorded with the close so an owed notice's retry
    -- replays this intent rather than the retrying caller's (ruling C32).
    close_bound TEXT,
    closed_at   INTEGER NOT NULL,
    closed_by   TEXT NOT NULL,
    close_why   TEXT NOT NULL
) STRICT;

CREATE INDEX closed_role_requests_by_project
    ON closed_role_requests (project, closed_at);

-- `pij requests` lists one project's requests in one state, newest first.
CREATE INDEX role_requests_by_project
    ON role_requests (project, state, created_at);

PRAGMA user_version = 37;
