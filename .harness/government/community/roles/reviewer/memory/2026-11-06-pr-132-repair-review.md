# PR 132 repair-delta review

Final verdict: **pass-with-notes** at `bd8331e09db997e1ab68938a037fc72d7791a29e`. Reviewed only the three-file 2851637..bd8331e delta. F005 closes: timeout_at uses the original deadline around prompt admission as well as event waiting; expiry reports bound and proceeds. Unchanged own_admission retains the service-owned continuation when its caller stops awaiting the reply. A timed-out prompt is not claimed cancelled. The five-second admission/300 ms bound regression passes in exact-head run 42153292528 (0.461 s); JUnit has 1,920 tests, zero failures/errors, all ten memory-turn HTTP cases. F001-F004/N001 remain closed. Fresh review-132-bd8331e stays frozen/clean; no local checks needed/run, no lock. Current report preserves the historical blocks below the final verdict.

Historical repair review follows.

Tags: reviewer, memory-turn, deadline, prompt-admission, CI-only

Hook: creating a deadline before an await does not bound that await.

Verdict: block at `2851637fd10be349d30329e0f49704eb12519f69`. Report: `../outputs/2026-11/pr-132-verdict.md`, current repair verdict before historical 5fbcfbd review.

F001-F004/N001 close: held actions capture proc/pane and skip replaced/tombstoned targets; closes recheck owner and CAS the reread; valid compact Receipt plus CLI raw-field restoration; unarmed watcher until matching outcome; fenced generated INDEX and escaped metadata; bound_ms on answers/reports/events. Both addendum cases are adopted and green. All holder actions are background/held; repeats get a named 409 with one pending turn per seat.

New F005 P2: run awaits delivery.accept without the held deadline, then applies timeout_at only to events.next. A short supported override (1 s) can expire while Claude UDS admission waits on its five-second HOLD_GRACE. The old action remains pending past its advertised bound; creating the deadline earlier merely accounts for that elapsed time once admission returns. Need one deadline covering both stages with cancellation-safe side effects and a delayed-admission CI case. Source trace only; no local reproduction claimed.

Exact-head run 42150037898 is 4/4 green, JUnit 1,919 tests zero failures/errors, including nine memory-turn HTTP tests and all reported repair regressions. Existing bound test admits the prompt promptly and tests only the later event wait. No local builds/tests/probes/mutations/harness gates; no build lock acquired. Fresh review-132-2851637 remains pinned and clean.

The ddocs availability limitation and performance-owned live AFTER proof remain explicit. Security owns its independent verdict; F005 is the advertised bounded-operation correctness issue.

**magicWand:** pause prompt admission beyond a short bound in CI, instead of assuming admission completed before starting the timer assertion.

**difficulties:** an earlier deadline timestamp looks like a full deadline fix, but its enforcement is confined to the subsequent stage. Passing promptly admitted event-timeout tests conceal the difference.
