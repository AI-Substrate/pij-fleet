-- Plan 164 review S3: an FYI's identity is (origin machine, msg_id), not msg_id.
-- A forwarded FYI from a paired machine shares the msg_id space with nothing
-- local, so `id TEXT PRIMARY KEY` silently dropped a local FYI whose id a remote
-- one had already taken (INSERT OR IGNORE), while the hold reported success.
-- `origin` is the sending machine's alias, '' for a local sender; NOT NULL
-- because it is part of the primary key. SQLite cannot alter a primary key, so
-- the table is rebuilt and every existing row is local.
CREATE TABLE fyis_by_origin (
  origin TEXT NOT NULL DEFAULT '',
  id TEXT NOT NULL,
  recipient TEXT NOT NULL,
  sender TEXT NOT NULL,
  body TEXT NOT NULL,
  held_at_ms INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'delivered', 'dropped')),
  settled_at_ms INTEGER,
  settled_via TEXT,
  PRIMARY KEY (origin, id)
);

INSERT INTO fyis_by_origin (origin, id, recipient, sender, body, held_at_ms, state, settled_at_ms, settled_via)
SELECT '', id, recipient, sender, body, held_at_ms, state, settled_at_ms, settled_via FROM fyis;

DROP TABLE fyis;
ALTER TABLE fyis_by_origin RENAME TO fyis;

CREATE INDEX fyis_pending_by_recipient ON fyis (recipient, held_at_ms, id) WHERE state = 'pending';

PRAGMA user_version = 26;
