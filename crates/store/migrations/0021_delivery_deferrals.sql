-- Diagnostic history survives retries, completion, and daemon restarts.
-- It never changes delivery eligibility or claims delivery success.
ALTER TABLE jobs ADD COLUMN deferral_reason TEXT;
ALTER TABLE jobs ADD COLUMN deferral_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN deferral_since_ms INTEGER;
ALTER TABLE jobs ADD COLUMN deferral_event_at_ms INTEGER;

PRAGMA user_version = 21;
