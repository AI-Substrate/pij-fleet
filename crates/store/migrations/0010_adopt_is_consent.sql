-- 0010 — adopting a Claude seat into pij is consent for inbound delivery.
--
-- Only UNKNOWN becomes accepted. An explicit false is an operator decision and
-- survives both registration and migration. Tombstoned rows stay historical.
UPDATE seats
SET cross_session_inbound_accept = 1
WHERE harness = 'claude'
  AND cross_session_inbound_accept IS NULL
  AND tombstoned_at IS NULL;

PRAGMA user_version = 10;
