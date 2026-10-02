-- 0011 — persist the harness-native session id when registration supplies it.
--
-- Existing seats predate this identity fact, so NULL is the only honest backfill.
ALTER TABLE seats ADD COLUMN harness_session TEXT;

PRAGMA user_version = 11;
