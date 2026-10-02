-- Pending reason transitions belong to the job's sampling clock, across restarts.
ALTER TABLE jobs ADD COLUMN deferral_reason_changes INTEGER NOT NULL DEFAULT 0;

PRAGMA user_version = 22;
