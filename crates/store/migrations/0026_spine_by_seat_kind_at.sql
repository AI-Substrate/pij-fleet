-- Plan 167: a seat's facts of one kind since a time, without reading its history.
CREATE INDEX spine_by_seat_kind_at ON spine_events (seat, kind, at);
PRAGMA user_version = 26;
