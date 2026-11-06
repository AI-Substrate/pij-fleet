# Run niced against the frozen review root. These are storage-boundary
# reproductions and controls, not an HTTP concurrency or notification run.
import sqlite3
import sys
from pathlib import Path

root = Path(sys.argv[1])
schema = (root / "crates/store/migrations/0036_community_outputs.sql").read_text()
insert = """
INSERT INTO community_outputs
  (path, project, role, kind, subject, head, result, writer, holds_role,
   answers, commit_sha, created_at)
VALUES (?, ?, 'reviewer', 'note', 'status', NULL, NULL, 'pij-writer', 1,
        NULL, 'governance-commit', 1)
ON CONFLICT (path) DO NOTHING
"""
shared = ".harness/government/community/roles/reviewer/outputs/2026-11/06-note-status.dd.json"

with sqlite3.connect(":memory:") as db:
    db.executescript(schema)
    assert db.execute(insert, (shared, "alpha")).rowcount == 1
    assert db.execute(insert, (shared, "alpha")).rowcount == 0
    assert db.execute(insert, (shared, "beta")).rowcount == 0
    alpha = db.execute(
        "SELECT COUNT(*) FROM community_outputs WHERE project = 'alpha'"
    ).fetchone()[0]
    beta = db.execute(
        "SELECT COUNT(*) FROM community_outputs WHERE project = 'beta'"
    ).fetchone()[0]
    assert (alpha, beta) == (1, 0)
    print("POSITIVE CONTROL: repeating one project/path remains idempotent")
    print("REPRODUCED: alpha and beta share a relative output path; beta is silently absent")
    assert db.execute(insert, (shared + "-different", "beta")).rowcount == 1
    assert db.execute(
        "SELECT COUNT(*) FROM community_outputs WHERE project = 'beta'"
    ).fetchone()[0] == 1
    print("POSITIVE CONTROL: beta is indexed when given a different path")

with sqlite3.connect(":memory:") as db:
    db.executescript((root / "crates/store/migrations/0023_fyis.sql").read_text())
    db.executescript((root / "crates/store/migrations/0027_fyi_origin.sql").read_text())
    hold = """
    INSERT OR IGNORE INTO fyis
      (origin, id, recipient, sender, body, held_at_ms, state)
    VALUES ('', ?, ?, 'pij-writer', ?, 1, 'pending')
    """
    # http/output.rs uses only writer.id and now_ms() in this identifier.
    first = f"output-pij-writer-{1700000000000}"
    second = f"output-pij-writer-{1700000000000}"
    assert first == second
    assert db.execute(hold, (first, "pij-asker-a", "output A")).rowcount == 1
    assert db.execute(hold, (second, "pij-asker-b", "output B")).rowcount == 0
    assert db.execute("SELECT recipient, body FROM fyis").fetchall() == [
        ("pij-asker-a", "output A")
    ]
    print("BOUNDARY REPRODUCED: equal notification timestamps collapse two distinct FYIs")
    assert db.execute(
        hold, ("output-pij-writer-1700000000001", "pij-asker-b", "output B")
    ).rowcount == 1
    print("POSITIVE CONTROL: a distinct notification ID preserves the second FYI")
