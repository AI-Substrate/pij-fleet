-- 0017 — daemon-owned detached jobs. 0014–0016 belong to governance.
-- Process identity and terminal facts survive daemon restarts; notification
-- delivery is acknowledged separately so unfinished delivery can be retried.
CREATE TABLE background_jobs (
    job_id         TEXT PRIMARY KEY NOT NULL,
    owner          TEXT NOT NULL,
    title          TEXT NOT NULL,
    command        TEXT NOT NULL,
    pid            INTEGER CHECK (pid > 0 AND pid <= 4294967295),
    proc_start     INTEGER CHECK (proc_start >= 0),
    pgid           INTEGER CHECK (pgid > 0 AND pgid <= 4294967295),
    out_path       TEXT NOT NULL,
    state          TEXT NOT NULL CHECK (state IN ('queued', 'running', 'done', 'killed', 'lost')),
    exit_code      INTEGER CHECK (exit_code BETWEEN -2147483648 AND 2147483647),
    started_at     INTEGER NOT NULL CHECK (started_at >= 0),
    finished_at    INTEGER CHECK (finished_at >= 0),
    kill_requested INTEGER NOT NULL DEFAULT 0 CHECK (kill_requested IN (0, 1)),
    notified       INTEGER NOT NULL DEFAULT 0 CHECK (notified IN (0, 1)),
    CHECK (
        (pid IS NULL AND proc_start IS NULL AND pgid IS NULL)
        OR (pid IS NOT NULL AND proc_start IS NOT NULL AND pgid IS NOT NULL)
    ),
    CHECK (state != 'queued' OR (pid IS NULL AND kill_requested = 0)),
    CHECK (state != 'running' OR pid IS NOT NULL),
    CHECK (
        (state IN ('queued', 'running') AND finished_at IS NULL AND exit_code IS NULL AND notified = 0)
        OR (state IN ('done', 'killed', 'lost') AND finished_at IS NOT NULL)
    )
) STRICT;

CREATE INDEX background_jobs_pending ON background_jobs (started_at, job_id)
    WHERE state IN ('queued', 'running') OR notified = 0;

PRAGMA user_version = 17;
