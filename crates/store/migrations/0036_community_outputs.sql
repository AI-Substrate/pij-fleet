-- Community outputs (plan 200, roadmap 001 s06; number from pm-001).
--
-- One row per `pij output`: the store's index of a pij/output document the
-- writer committed on the governance branch, which stays the truth. `subject`
-- and `head` answer `pij outputs --subject pr:N --head <sha>` for that exact
-- head; `answers` is the request it answers (ruling C3), when there is one.
CREATE TABLE community_outputs (
    path        TEXT PRIMARY KEY,
    project     TEXT NOT NULL,
    role        TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('verdict', 'deliverable', 'note', 'handover')),
    subject     TEXT NOT NULL,
    head        TEXT,
    result      TEXT,
    writer      TEXT NOT NULL,
    holds_role  INTEGER NOT NULL CHECK (holds_role IN (0, 1)),
    answers     TEXT,
    commit_sha  TEXT NOT NULL,
    created_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX community_outputs_subject ON community_outputs (project, subject, head);
CREATE INDEX community_outputs_role ON community_outputs (project, role, created_at);

PRAGMA user_version = 36;
