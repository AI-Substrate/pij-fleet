-- Seat capabilities: the secured issuer (plan 223 AC2; plan 221 workshop 001
-- contracts a and b).
--
-- Only digests are stored: SHA-256 of 32 random bytes, never the secret. Times
-- are epoch milliseconds.
--
-- `seat_nonces`: one row per spawn or revive, written before the launch. A
-- registration consumes it in the same transaction that writes the seat and its
-- capability; a consumed row stays until it expires so a second presentation is
-- refused E-RS-SPAWN-NONCE-USED. Expired rows are dropped when their spawn is
-- next touched and once at daemon start. There is no sweep. `pane_pid` and
-- `pane_start` are the spawn's pane process, recorded once the launch has a pane:
-- a nonce registration must come from inside that pane (review D5).
CREATE TABLE seat_nonces (
    spawn_id    TEXT PRIMARY KEY,
    seat        TEXT NOT NULL,
    harness     TEXT NOT NULL,
    digest      BLOB NOT NULL,
    issued_at   INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    consumed_at INTEGER,
    pane_pid    INTEGER,
    pane_start  TEXT
) STRICT;

-- `seat_capabilities`: one row per live incarnation. The boundary is the seat's
-- process at issue (`seats.pid`, `seats.proc_start`); the owner is the process
-- that bootstrapped it, when observed. Every seat write that tombstones the
-- seat or changes its process deletes the row in the same transaction.
-- `prev_digest` is two-phase rotation's old token, valid until the new one is
-- first presented. Process starts are kept as decimal text.
CREATE TABLE seat_capabilities (
    seat           TEXT PRIMARY KEY,
    owner_pid      INTEGER,
    owner_start    TEXT,
    boundary_pid   INTEGER,
    boundary_start TEXT,
    session        TEXT,
    digest         BLOB NOT NULL,
    prev_digest    BLOB,
    issued_at      INTEGER NOT NULL,
    via            TEXT NOT NULL CHECK (via IN ('nonce', 'peer', 'rotation'))
) STRICT;

-- `seat_reissue_marks`: a seat whose capability was revoked by an UNPROVEN
-- tombstone (a close, or any tombstone proven neither by the seat's own
-- capability nor by its owner's observed death), and why. While marked, the
-- seat's capability is re-issued only by a spawn nonce (spawn or revive) or its
-- current capability, never by a peer bootstrap (E-RS-SEAT-REISSUE-NONCE). The
-- mark is written in the transaction that revokes, and a nonce or capability
-- re-issue clears it in the transaction that issues.
CREATE TABLE seat_reissue_marks (
    seat      TEXT PRIMARY KEY,
    reason    TEXT,
    marked_at INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 47;
