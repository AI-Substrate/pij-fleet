# PR 132 role-memory review

Tags: reviewer, memory-turn, incarnation, typed-client, event-order, CI-only

Hook: a bounded asynchronous action must preserve incarnation, response type and event-order authority across its wait.

Verdict: block at `5fbcfbdc0323af2546c0c9b17a89e3a0f0e2ae22`. Report: `../outputs/2026-11/pr-132-verdict.md`. Three P2s are source-proven, not locally reproduced.

F001: after the memory wait, self-close and parent-close call close_seat(expected=None). The helper re-reads the current descriptor and CAS-tombstones it, so an old pending close can close a revived incarnation; delayed compact similarly carries only the seat ID. The missing incarnation veto is a one-direction safety brake.

F002: memory_turn::held returns seat/held/memory_turn, but native compact-self/send_control still decode Envelope<Receipt>, requiring msg_id/outcome/at. The client reports an adapter payload-shape failure although the daemon scheduled work. Inline compact's memory report also disappears when Command::Send clears raw_json and serializes the old Receipt type.

F003: subscribe_live starts before send. If admission returns Delivered, Watch starts delivered=true and accepts an old idle buffered before the prompt's delivery.outcome. Sequence ordering must establish the delivery boundary before idle can finish the turn.

N001 P3: the approved bound_ms field is missing from held and completed receipts. Rust CLI has no installed request timeout; no full-duration runtime proof claimed. Ordinary one-delivery/live-subscription/no-poll paths, non-holder bypass, memory writer/index behavior and requested docs flips are present.

Exact-head CI run 42142573572 passes all four jobs. Downloaded JUnit shows 1,909 tests with zero failures/errors, including five memory-turn HTTP cases, four watcher cases and memory storage/concurrent-writer cases. The new tests use Value at the HTTP boundary and do not cover the native typed response. ddocs CI explicitly warns UNAVAILABLE; a passing skip-on-missing real-ddocs testcase is not validator execution.

No local builds, tests, probes, mutation checks, harness gates or live-runtime work under pm-001's backoff. No lock acquired, no product changes, commits or pushes. Worktree fleet-review-132 remains frozen; fleet-review-130 was not moved.

Follow-up while awaiting PM's forthcoming head: unchanged memory-note logic has F004 P2 in `../outputs/2026-11/pr-132-memory-notes-addendum.md`. Generated-row detection strips arbitrary hand-written adjacent `.dd.md` links, and a valid title containing `[delivery](flow)` produces a row the detector fails to recognize, duplicating it on every regeneration. Source/input trace only, no execution. This addendum is not a verdict on the forthcoming head; the old posted verdict remains pinned to 5fbcfbd.

PM's approved revision, awaiting a new exact head and fresh worktree: capture proc identity at hold time and skip delayed actions if it changed or the seat is tombstoned; carry bound_ms in every memory_turn receipt and spine event; hold every holder compact and run it in the background so the 65-second Copilot client cannot cut off an inline wait. These are recorded as found/owed, not verified fixes. Also recheck the native held-response contract, buffered-idle ordering and F004 against that new source. Do not update the posted verdict until PM supplies the new review target.

**magicWand:** a CI lifecycle matrix crossing self/parent, typed CLI/raw HTTP, queued/direct delivery, buffered old idle and death/revive.

**difficulties:** green HTTP tests conceal typed-client incompatibility; an admitted-send receipt can invalidate event chronology if used to seed a watcher; CI ddocs success means unavailable, not validated. Repair should encode those boundaries without a local reviewer build window.
