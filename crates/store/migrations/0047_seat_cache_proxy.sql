-- Cache-proxy route per seat (plan 229, workstream 004 s03).
--
-- The spawn gate's decision for a Copilot seat, as JSON
-- `{"state":"proxied"|"plain","reason":..,"fix":..}`. NULL for every other
-- harness, and for a Copilot seat launched before the gate, which reads as
-- plain. Written at spawn and revive; registration keeps it.
ALTER TABLE seats ADD COLUMN cache_proxy TEXT;

PRAGMA user_version = 47;
