-- Time limits on role requests (plan 205, roadmap 001 s12; number from pm-001).
--
-- `checkin_at`: when the holder was sent its one check-in. `escalated_at`:
-- when the PA (else the prime) was told the check-in went unanswered.
-- `answered_at`: set by s06 when an output answers the request.
-- `limit_claim_at`: a check-in or notice claimed but not yet admitted. The
-- stamps are written only once delivery admits the message; a failed attempt
-- clears the claim, and one a stopped daemon left behind expires. The watchdog
-- round reads only requests not yet escalated or answered, through the
-- partial index, so its cost follows the open limited requests, never history.
ALTER TABLE role_requests ADD COLUMN checkin_at INTEGER;
ALTER TABLE role_requests ADD COLUMN escalated_at INTEGER;
ALTER TABLE role_requests ADD COLUMN answered_at INTEGER;
ALTER TABLE role_requests ADD COLUMN limit_claim_at INTEGER;

CREATE INDEX role_requests_limited ON role_requests (delivered_at)
    WHERE state = 'delivered' AND limit_ms IS NOT NULL
      AND escalated_at IS NULL AND answered_at IS NULL;

PRAGMA user_version = 35;
