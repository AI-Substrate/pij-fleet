-- Dedupe is per DESTINATION, not global (review F5).
--
-- The old index was `dedupe_key` alone over live rows, and both delivery and
-- federation rows use the caller's `msg_id`. So two sends with one msg_id to two
-- different recipients collapsed: the second INSERT hit ON CONFLICT DO NOTHING,
-- the read-back returned the FIRST row, and the route answered Queued for a
-- message that was never going anywhere. User-reachable silent loss, widened
-- cross-machine by wave 4.
--
-- `kind` is `delivery:<seat>` / `federation:<machine>` — the federation half was
-- ONE kind for every peer until review round 2 probed it — so scoping by it makes the
-- rule what it was always meant to be: N rapid submits of ONE message to ONE
-- recipient collapse to one row; the same id to a different recipient is a
-- different message.
DROP INDEX IF EXISTS jobs_dedupe_live;
CREATE UNIQUE INDEX jobs_dedupe_live
    ON jobs (kind, dedupe_key)
    WHERE state IN ('pending', 'running');
