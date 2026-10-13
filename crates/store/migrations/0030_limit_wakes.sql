-- PA watchdog: usage-limit reset (plan 172 R3, 2026-10-13).
--
-- limit_wakes: one usage-limit-reset wake per (scope, resets_at). The row is
-- claimed before the wake is sent, so several seats stopped by the same limit,
-- concurrent rounds or a daemon restart can never wake twice. `resets_at` is
-- the provider's absolute UTC epoch in seconds, never a wall-clock string.
CREATE TABLE limit_wakes (
    scope         TEXT NOT NULL,
    resets_at     INTEGER NOT NULL,
    limit_type    TEXT NOT NULL,
    target        TEXT,
    target_source TEXT NOT NULL,
    fired_at      INTEGER NOT NULL,
    PRIMARY KEY (scope, resets_at)
) STRICT;

PRAGMA user_version = 30;
