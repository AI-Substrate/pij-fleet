-- Plan 167: one seat's facts of one kind after an (at, seq) cursor, without
-- reading its history or re-reading earlier entries at the cursor's own time.
CREATE INDEX spine_by_seat_kind_at ON spine_events (seat, kind, at, seq);
PRAGMA user_version = 26;
