-- Status declarations should skip unrelated per-seat event history.
CREATE INDEX spine_by_seat_kind ON spine_events (seat, kind, seq);
PRAGMA user_version = 20;
