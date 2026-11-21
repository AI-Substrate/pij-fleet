-- Seats by pane (F258 cut B1).
--
-- `whoami --pane` (the status lines: about 8 calls/s on a fleet of 76 Copilot
-- sessions) and adopt read the rows that record one pane. Without an index
-- each read scanned every seat row ever written: 3,212 on the live store,
-- 0.65 ms of SQLite CPU per call. Pane ids repeat across tmux servers and
-- incarnations, so one pane can carry many rows; this makes the read one seek.
-- The registry's list query takes a pane-led branch so SQLite can use it
-- (crates/store/src/registry.rs, `LIST_BY_PANE_SQL`).
CREATE INDEX seats_by_pane ON seats (pane);

PRAGMA user_version = 48;
