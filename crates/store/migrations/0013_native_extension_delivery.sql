-- 0013 — native Copilot extension delivery replaces RPC launch intent.
-- Existing rows retain their history but require fresh native attestation.
ALTER TABLE seats DROP COLUMN rpc_port_intent;
ALTER TABLE seats ADD COLUMN native_extension_delivery INTEGER NOT NULL DEFAULT 0
    CHECK (native_extension_delivery IN (0, 1)
        AND (native_extension_delivery = 0 OR (
            harness = 'copilot'
            AND pid IS NOT NULL AND pid BETWEEN 1 AND 4294967295
            AND proc_start IS NOT NULL AND proc_start > 0
            AND harness_session IS NOT NULL AND length(harness_session) > 0
            AND tombstoned_at IS NULL
        )));

PRAGMA user_version = 13;
