-- Extension delivery lease failures are independent of ordinary queue retries.
ALTER TABLE jobs ADD COLUMN lease_expirations INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 18;
