-- R4-AMEND-1: a claimed job can return to pending, counted and scheduled.
--
-- 0002 said "a job never returns to pending: a retry is a NEW row". Two wave-4
-- units found independently that this makes retry unimplementable: ack-then-
-- enqueue has a crash window that LOSES the body, and enqueue-then-ack collapses
-- onto the still-live row it means to replace. So the transition becomes real,
-- and it is atomic in one place rather than reconstructed by callers.
--
-- `attempt` exists here and NOWHERE ELSE. The fork split attempt counting between
-- its daemon and its consumers; that split is the root cause of its still-open
-- G25, where the counter never moved, `parked` was unreachable, and pointers
-- re-announced every 90 seconds for ever. One column, one writer.
ALTER TABLE jobs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0;

-- Not-before, so backoff is a fact in the row rather than a sleep in a worker.
-- A worker that expresses delay by sleeping is holding a claim it is not using,
-- and nothing else can tell the difference between that and a hung worker.
ALTER TABLE jobs ADD COLUMN not_before INTEGER NOT NULL DEFAULT 0;
