-- F280: the alternate launch a seat was started with (`pij-rs spawn --alt`),
-- as JSON `{"name":..,"home":..}`. NULL for a default launch. Written at
-- spawn and revive; registration keeps it; revive reads it back.
ALTER TABLE seats ADD COLUMN alt TEXT;

PRAGMA user_version = 51;
