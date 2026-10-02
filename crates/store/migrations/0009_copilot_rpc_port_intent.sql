-- 0009 — Copilot RPC launch intent.
--
-- This is not listener evidence. A value means only that spawn emitted
-- `--ui-server --port N`; NULL means it did not, including every pre-migration
-- seat. Listener possession is corroborated from live process identity at read
-- time by the transport, never inferred from this column.
ALTER TABLE seats ADD COLUMN rpc_port_intent INTEGER
    CHECK (rpc_port_intent IS NULL OR rpc_port_intent BETWEEN 1 AND 65535);

PRAGMA user_version = 9;
