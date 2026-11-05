-- Community directory pointer (plan 190, roadmap 001 s02, claimed by pm-001).
--
-- The governance branch holds the community directory; this table only points
-- at it: for each document pij wrote, the project, the path relative to the
-- governance worktree, and the commit that last wrote it. It is rebuildable
-- from the branch head, so it carries nothing the branch does not.
CREATE TABLE community_pointers (
    project    TEXT NOT NULL,
    path       TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    PRIMARY KEY (project, path)
) STRICT;

PRAGMA user_version = 31;
