-- 0011 down — return to the schema-10 seat row.
ALTER TABLE seats DROP COLUMN harness_session;

PRAGMA user_version = 10;
