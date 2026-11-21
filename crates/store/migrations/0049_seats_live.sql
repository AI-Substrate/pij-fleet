-- Live seats (F258).
--
-- The pane observer lists live seats every 600 ms and the death sweep every
-- 5 s. Every seat row ever written stays in `seats` (3,212 on the live store,
-- about 190 live), and `tombstoned_at` had no index, so each of those reads
-- scanned the whole table for the few live rows. This partial index holds only
-- the live rows, in id order: the registry's live-only list walks it instead
-- (crates/store/src/registry.rs, `LIST_LIVE_SQL`). Archiving retired rows is
-- F259's, not this.
CREATE INDEX seats_live ON seats (id) WHERE tombstoned_at IS NULL;

PRAGMA user_version = 49;
