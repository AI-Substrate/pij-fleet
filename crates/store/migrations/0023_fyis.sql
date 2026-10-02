-- Plan 158: an FYI waits for its recipient's next real turn instead of opening one.
-- `pending` rows ride along appended to the next non-control delivery, or are
-- claimed by a typed-turn hook. Each row moves pending -> delivered exactly once
-- (a failed carrier may restore it to pending) or pending -> dropped at tombstone.
CREATE TABLE fyis (
  id TEXT PRIMARY KEY,
  recipient TEXT NOT NULL,
  sender TEXT NOT NULL,
  body TEXT NOT NULL,
  held_at_ms INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'dropped')),
  settled_at_ms INTEGER,
  settled_via TEXT
);

CREATE INDEX fyis_pending_by_recipient ON fyis (recipient, held_at_ms, id) WHERE state = 'pending';

PRAGMA user_version = 23;
