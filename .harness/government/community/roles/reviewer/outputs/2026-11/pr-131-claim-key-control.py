import re
import sqlite3
from pathlib import Path

schema = Path("crates/store/migrations/0033_claims_deliveries.sql").read_text()


def rejects_duplicate(sql):
    with sqlite3.connect(":memory:") as db:
        db.executescript(sql)
        insert = (
            "INSERT INTO claims (project, kind, number, seat, subject, claimed_at) "
            "VALUES (?, ?, 196, 'reviewer', 'control', 1)"
        )
        db.execute(insert, ("alpha", "plan"))
        db.execute(insert, ("beta", "plan"))
        db.execute(insert, ("alpha", "prd"))
        assert db.execute("SELECT COUNT(*) FROM claims").fetchone()[0] == 3
        try:
            db.execute(insert, ("alpha", "plan"))
        except sqlite3.IntegrityError:
            return True
        return False


assert rejects_duplicate(schema), "the exact migration must reject duplicate keys"
mutant, count = re.subn(
    r",\n    PRIMARY KEY \(project, kind, number\)", "", schema
)
assert count == 1, "the mutation must remove exactly the claims key"
assert not rejects_duplicate(mutant), "the oracle must detect the missing key"
print("PASS: exact migration rejects duplicate (project, kind, number)")
print("PASS: same number in another project or kind is accepted")
print("DETECTED: removing the claims primary key permits the duplicate")
