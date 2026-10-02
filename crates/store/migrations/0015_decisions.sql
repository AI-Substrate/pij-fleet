-- A prepared outbound answer remains open until delivery is durably accepted.
CREATE TABLE decisions (
    id TEXT PRIMARY KEY NOT NULL,
    asked_by TEXT NOT NULL,
    parent TEXT,
    question TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'answered')),
    asked_at INTEGER NOT NULL CHECK (asked_at >= 0),
    question_seq INTEGER NOT NULL REFERENCES spine_events(seq),
    answer TEXT,
    answered_by TEXT,
    answered_at INTEGER,
    answer_msg_id TEXT UNIQUE,
    answer_seq INTEGER REFERENCES spine_events(seq),
    CHECK ((answer IS NULL) = (answered_by IS NULL)),
    CHECK ((state = 'answered') = (answered_at IS NOT NULL)),
    CHECK ((state = 'answered') = (answer_seq IS NOT NULL)),
    CHECK (state != 'answered' OR answer IS NOT NULL)
);
CREATE INDEX decisions_open_asked ON decisions(state, asked_at, id);
CREATE INDEX decisions_asker ON decisions(asked_by);
