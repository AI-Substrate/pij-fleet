-- 0014 — extend the existing orchestration rows, preserving unknown legacy facts.
ALTER TABLE projects ADD COLUMN description TEXT;
ALTER TABLE projects ADD COLUMN repo TEXT;
ALTER TABLE projects ADD COLUMN plan_path TEXT;
ALTER TABLE projects ADD COLUMN prime_id TEXT;

-- A legacy reservation cannot attest that Git creation actually succeeded.
ALTER TABLE streams ADD COLUMN state TEXT NOT NULL DEFAULT 'reserved'
    CHECK(state IN ('reserved', 'created', 'closed'));
UPDATE streams SET id = project || ':' || slug;

ALTER TABLE batons ADD COLUMN resource TEXT;
ALTER TABLE batons ADD COLUMN probe TEXT;
ALTER TABLE batons ADD COLUMN repo TEXT;
ALTER TABLE prime_designation ADD COLUMN state TEXT NOT NULL DEFAULT 'current'
    CHECK(state IN ('current', 'retired'));

-- The old CHECK admits only pending/acknowledged. Rebuild this table in the
-- migration transaction rather than weakening the check or losing old bodies.
CREATE TABLE dispatches_governance (
    id              TEXT PRIMARY KEY NOT NULL,
    from_seat       TEXT NOT NULL,
    to_seat         TEXT NOT NULL,
    packet_path     TEXT NOT NULL,
    packet_sha256   TEXT,
    msg_id          TEXT UNIQUE,
    state           TEXT NOT NULL CHECK(state IN ('queued', 'delivered', 'acked')),
    created_at      INTEGER NOT NULL,
    delivered_at    INTEGER,
    acknowledged_at INTEGER,
    ack_seat        TEXT,
    ack_sha256      TEXT,
    ack_at          INTEGER,
    canary_nonce    TEXT,
    canary_model    TEXT,
    canary_passed_at INTEGER,
    canary_evaluator TEXT,
    CHECK((ack_seat IS NULL AND ack_sha256 IS NULL AND ack_at IS NULL)
       OR (ack_seat IS NOT NULL AND ack_sha256 IS NOT NULL AND ack_at IS NOT NULL
           AND state = 'acked' AND ack_seat = to_seat
           AND packet_sha256 IS NOT NULL AND ack_sha256 = packet_sha256
           AND acknowledged_at IS NOT NULL AND ack_at = acknowledged_at)),
    CHECK((canary_nonce IS NULL AND canary_model IS NULL
           AND canary_passed_at IS NULL AND canary_evaluator IS NULL)
       OR (canary_nonce IS NOT NULL AND canary_model IS NOT NULL
           AND canary_passed_at IS NOT NULL AND canary_evaluator IS NOT NULL
           AND ack_seat IS NOT NULL))
) STRICT;
INSERT INTO dispatches_governance
    (id, from_seat, to_seat, packet_path, state, created_at, acknowledged_at)
SELECT id, from_seat, to_seat, body,
       CASE state WHEN 'pending' THEN 'queued' WHEN 'acknowledged' THEN 'acked' END,
       created_at, acknowledged_at
FROM dispatches;
DROP TABLE dispatches;
ALTER TABLE dispatches_governance RENAME TO dispatches;
CREATE INDEX dispatches_by_assignee ON dispatches (to_seat, state);

CREATE TABLE fences (
    id          TEXT PRIMARY KEY NOT NULL,
    stream      TEXT NOT NULL UNIQUE REFERENCES streams(id),
    paths       TEXT NOT NULL CHECK(json_valid(paths) AND json_type(paths) = 'array'),
    shared      TEXT NOT NULL CHECK(json_valid(shared) AND json_type(shared) = 'array'),
    declared_by TEXT NOT NULL,
    declared_at INTEGER NOT NULL
) STRICT;

CREATE TABLE task_assignments (
    id           TEXT PRIMARY KEY NOT NULL,
    node_id      TEXT NOT NULL,
    task         TEXT NOT NULL,
    project      TEXT REFERENCES projects(slug),
    opened_by    TEXT NOT NULL,
    opened_at    INTEGER NOT NULL,
    closed_at    INTEGER,
    close_reason TEXT CHECK(close_reason IN ('done', 'cancelled', 'failed', 'superseded')),
    CHECK((closed_at IS NULL AND close_reason IS NULL)
       OR (closed_at IS NOT NULL AND close_reason IS NOT NULL))
) STRICT;
CREATE INDEX task_assignments_by_node ON task_assignments (node_id, opened_at, id);

CREATE TABLE plan_attestations (
    seat        TEXT PRIMARY KEY NOT NULL,
    plan_id     TEXT NOT NULL,
    attested_by TEXT NOT NULL,
    attested_at INTEGER NOT NULL
) STRICT;

CREATE TABLE baton_requests (
    id           TEXT PRIMARY KEY NOT NULL,
    baton        TEXT NOT NULL REFERENCES batons(name),
    requester    TEXT NOT NULL,
    purpose      TEXT NOT NULL,
    pin          TEXT,
    evidence     TEXT,
    requested_at INTEGER NOT NULL,
    state        TEXT NOT NULL CHECK(state IN ('requested', 'granted', 'returned', 'reclaimed'))
) STRICT;
CREATE INDEX baton_requests_by_baton ON baton_requests (baton, requested_at, id);
ALTER TABLE baton_leases ADD COLUMN request_id TEXT REFERENCES baton_requests(id);
CREATE UNIQUE INDEX baton_lease_request ON baton_leases(request_id) WHERE request_id IS NOT NULL;
ALTER TABLE baton_lease_history ADD COLUMN request_id TEXT;
ALTER TABLE baton_lease_history ADD COLUMN actor TEXT;
ALTER TABLE baton_lease_history ADD COLUMN evidence TEXT;
ALTER TABLE baton_lease_history ADD COLUMN release_kind TEXT
    CHECK(release_kind IN ('returned', 'reclaimed'));

PRAGMA user_version = 14;
