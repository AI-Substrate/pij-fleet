# UDS fixtures

`status-held.ndjson` and `status-denied.ndjson` are RECORDINGS, not constructions:
lines captured off the wire from Claude 2.1.250 on 2026-08-31 (plan 110; the run
is written up in `scratch/woodpecker-poc/FINDINGS.md`). Only the recipient socket
path is sanitised to the fixture pid; the shape, the field set, the wording and
the fresh-uuid `msg_id` are exactly as the CLI emitted them.

That matters here specifically. The previous fixtures were INVENTED in the shape
fork issue #311 documented (`{"type":"peer_message_status","orig_msg_id":…,
"wereHeld":true}`), a shape that never appeared once across five measured arms —
so the suite was green over a decoder production could never satisfy. A fixture
that its own consumer authored proves nothing about the wire.

`status-dropped.ndjson` remains UNRECORDED — no drop was ever observed — and the
decoder path it exercises is therefore unproven against a live build. It is kept
because refusing to upgrade an unknown status is still the honest default.
