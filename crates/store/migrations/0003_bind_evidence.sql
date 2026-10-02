-- 0003 — bind evidence on the seat row (R5 items 13-15).
--
-- The class these columns close: the registry row lacked facts another surface
-- already held. `pij whoami` knew a seat's folder while its descriptor read
-- null; a spawn's argv and the pane footer both knew the model while the row did
-- not. A fact that lives in one instrument and not in the authority is a fact
-- nobody can act on — and it is how 185 of 215 rows machine-wide ended up
-- `unadopted` with nobody able to say who governed them.
--
-- All four are nullable, deliberately: a seat adopted by a human never had a
-- spawn id, and a row that invented one would be the same lie in the other
-- direction. Absent stays absent.

ALTER TABLE seats ADD COLUMN spawn_id TEXT;
ALTER TABLE seats ADD COLUMN model TEXT;
ALTER TABLE seats ADD COLUMN provider TEXT;
ALTER TABLE seats ADD COLUMN effort TEXT;

-- Equivalence reconciliation (u-orchestration) looks seats up by the launch they
-- came from; without this it is a table scan per lookup on a table that grows
-- with every spawn the machine has ever done.
CREATE INDEX seats_by_spawn ON seats (spawn_id);

PRAGMA user_version = 3;
