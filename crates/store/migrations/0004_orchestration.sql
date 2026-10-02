-- 0004 — durable orchestration state.
--
-- Every multi-row mutation in the adapter uses BEGIN IMMEDIATE. Constraints are
-- the final line of defence: one live baton holder and one stream ordinal.

CREATE TABLE spawn_records (
    spawn_id     TEXT PRIMARY KEY NOT NULL,
    spawner      TEXT NOT NULL,
    recorded_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE descriptor_merge_history (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    survivor_id      TEXT NOT NULL,
    alias_id          TEXT NOT NULL,
    survivor_json    TEXT NOT NULL,
    alias_json        TEXT NOT NULL,
    mismatched_fields TEXT NOT NULL,
    verified_at      INTEGER NOT NULL
) STRICT;

CREATE TABLE projects (
    slug        TEXT PRIMARY KEY NOT NULL,
    created_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE streams (
    id          TEXT PRIMARY KEY NOT NULL,
    project     TEXT NOT NULL REFERENCES projects(slug),
    ordinal     INTEGER NOT NULL UNIQUE,
    slug        TEXT NOT NULL,
    branch      TEXT NOT NULL UNIQUE,
    worktree    TEXT NOT NULL UNIQUE,
    base_ref    TEXT NOT NULL,
    created_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    UNIQUE(project, slug)
) STRICT;

CREATE TABLE batons (
    name        TEXT PRIMARY KEY NOT NULL,
    description TEXT NOT NULL,
    created_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;

CREATE TABLE seat_roles (
    seat        TEXT PRIMARY KEY NOT NULL,
    role        TEXT NOT NULL,
    assigned_by TEXT NOT NULL,
    assigned_at INTEGER NOT NULL
) STRICT;

CREATE TABLE prime_designation (
    singleton     INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    seat          TEXT NOT NULL UNIQUE,
    designated_by TEXT NOT NULL,
    designated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE dispatches (
    id              TEXT PRIMARY KEY NOT NULL,
    from_seat       TEXT NOT NULL,
    to_seat         TEXT NOT NULL,
    body            TEXT NOT NULL,
    state           TEXT NOT NULL CHECK(state IN ('pending', 'acknowledged')),
    created_at      INTEGER NOT NULL,
    acknowledged_at INTEGER
) STRICT;

CREATE INDEX dispatches_by_assignee ON dispatches (to_seat, state);

CREATE TABLE baton_leases (
    baton       TEXT PRIMARY KEY NOT NULL,
    holder      TEXT NOT NULL,
    lease_id    TEXT NOT NULL UNIQUE,
    acquired_at INTEGER NOT NULL
) STRICT;

CREATE TABLE baton_lease_history (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    baton       TEXT NOT NULL,
    holder      TEXT NOT NULL,
    lease_id    TEXT NOT NULL,
    action      TEXT NOT NULL CHECK(action IN ('claimed', 'released')),
    at          INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 4;
