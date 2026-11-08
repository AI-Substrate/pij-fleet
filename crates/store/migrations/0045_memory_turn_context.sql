-- The memory turn's context, read in bounded time (F130, #184 review P184-F001).
--
-- A compact or close asks a seat to write memory for every role it held in its
-- current context: the open holdings, and those released since its last
-- delivered compact. Two indexed reads, both bounded:
--
-- 1. the seat's newest compaction rows, newest first by admission order, to
--    find the last one settled as delivered (`woken`, `resumed` or
--    `holds-nothing`); the pending index covers only unsettled rows;
-- 2. the seat's holdings by release time, newest first. The open-holdings
--    index is partial (`to_at IS NULL`), and nothing else led with the seat.
CREATE INDEX compaction_wakes_by_seat ON compaction_wakes (seat, seq);

CREATE INDEX community_assignments_by_seat_released
    ON community_assignments (seat, to_at);

PRAGMA user_version = 45;
