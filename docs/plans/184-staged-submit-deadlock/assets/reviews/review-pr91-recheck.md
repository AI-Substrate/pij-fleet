# Recheck: PR #91 at `9edfee0` (rebased onto main 8687560)

Reviewer: `pij-review-pr91`. Read-only. Scratch tests were removed afterwards and the worktree is clean.

**Verdict: merge. 0 blockers.** One should-fix (the S3 wrap-fit rule) and one operational note (walrus). Neither lets pij delete non-whitespace.

The rebase is clean: `git range-diff` shows commits 1–4 unchanged (`=`). The only new commit is `6c1fe6e`.

## 1. B1: fixed
- **Typing never counts.** `counts_toward_limit` (`crates/daemon/src/pointer/worker.rs:~58`) counts only composer-busy and unrecognized. A typing hold retries at the gate's `retry_after`, not on the next tick.
- **The clock is per job.** A `Held{since_ms, holds}` map is kept per job in memory, never derived from `attempt`. Commands fail at 7 minutes, bodies at 20.
- **The walrus test is faithful on its key facts.** It uses the real `SqliteQueue`, `set_attempt(head, 9_180)`, the default 60 s grace on an injected clock, and asserts that the first tick is held and not reserved (`wait_s` 55–61). Past the grace it asserts `staged-unsent` with the sender told.
  - Caveat: the six rows behind the head are **bodies over the socket**. Walrus's real queue is mostly stale `/compact` from eagle (diagnosis). After this fix, the clear leaves a blank composer, and those compacts are then typed into eagle's seat one after another. Before you bounce, have eagle's prime cancel or release them, or accept that they will run. This is operational, not code.
- **Nit: typing time still runs the clock.** Once a counted hold has started the clock, later typing time is included, and the clock is reset only on ack or fail. A human who alternates drafting and pausing can fail a body at 20 minutes. Acceptable (the sender is told); stating it so nobody is surprised.

## 2. S3: non-whitespace is never deleted; the wrap-fit rule is weaker than documented (should-fix)
**What now holds:**
- The decision is made on the read under ownership (S2 witness).
- A caret check guards the last row: `composer_region` clips the caret row at the caret, so trailing spaces typed there make the row shorter than the caret position, and the match is refused.
- The composer is re-read after a clear (`reads_blank`, after a 250 ms settle). If it does not read blank, nothing more is typed and the delivery fails "left for the operator".

Why non-whitespace cannot be deleted:
- `BSpace` only deletes backwards from the caret.
- Every visible character before the caret must equal pij's leftover, in order.
- The caret must sit at the end of the leftover.

So the only extra characters are ones the capture cannot show (whitespace), and the backspace count is at most the leftover's length. Text after the caret is never touched. Extra backspaces into an empty composer do nothing.

**The gap:** the wrap-fit rule measures against the *widest row shown*, and a typed newline shrinks that row. A typed newline in the second half of the widest line is still accepted as a soft wrap:
```
REVIEW typed newline at "delivery staged": accepted=false
REVIEW typed newline at "inspect the":     accepted=true
REVIEW typed newline at "and press":       accepted=true
REVIEW typed newline at "press Ctrl-C":    accepted=true
REVIEW typed newline at "clear it":        accepted=true
```
(Scratch: the walrus rendering with `"{a}\n  {b}"` substituted, `is_staged_leftover`.)

- **Outcome:** the human's newline is deleted, a `/` is left behind, the re-read sees residue, and the delivery fails by name. That is the same class as the documented residual (whitespace only, ends by name), but wider than the docs say. The docs claim the width rule turns this into a human draft.
- **Likelihood:** tiny. The human must type a newline inside pij's notice, move the caret back to the end, and then stay idle for 60 s.
- **Fix:** take the width from the composer's rule row or the pane width, not the widest content row. Or say in the docs that the residual includes mid-line newlines.

**Acceptable?** Yes. Only whitespace can go, it needs a deliberate edit to pij's own notice, and the delivery ends by name instead of retrying.

**Unverified live:** that Claude's real tmux cursor sits at its drawn caret. If it doesn't, recognition fails, which is the safe direction: a hold that ends by name.

## 3. S4: fine
- `StagedSubmit.pasted` is appended only after each chunk pastes successfully, and `paste_open` closes an unterminated bracketed paste before the notice.
- The gate records `staged_leftover(pasted)` per pane, keyed by text, and forgets it on an idle look or a clear. It also matches the full-text form, which survives a restart.
- After a restart, a prefix leftover is unrecognised, so it is held as composer-busy: a command fails `hold-limit` at 7 minutes; a body gets one notice at 7 minutes and fails at 20. Both end by name.
- A restart restarts the in-memory clock, which only lengthens the hold.

## 4. Held bodies: safe
- At 7 minutes there is one `delivery.hold-limit` fact. It now carries `msg_id`, so the lookup dedupes, and that is proven on the real queue.
- At 20 minutes, `fail_pane_delivery` acks the job Failed and publishes `delivery.parked`, and the park notice tells the sender: "was not delivered … Resend".
- Nothing is lost silently.
- Only the **typed** fallback is affected; socket bodies never reach this path.
- The cost is that the sender must resend. A seat sitting at a permission prompt or another unrecognised UI for 20 minutes will now fail its typed bodies by name. That is a deliberate trade, and I agree with it.

## 5. Plan 186 interaction: intact
- Copilot seats list `control_kind` even when paneless (its S1 sweep kept, `worker.rs:~185-198`).
- A harness/kind mismatch still defers as `native-consumer-owned`, and a paneless claim still goes to `release_or_backoff`.
- The settled-idle brake (`copilot_idle_settled`, two reads 250 ms apart) runs **before** `submit_command_claim`. Copilot controls therefore share the clear, the count and the notice only after the brake. `recipient-busy` releases do not count toward the limit.
- Copilot's two-line leftover reads `unrecognized`, which counts, so it ends `hold-limit` at 7 minutes (tested).

## 6. Anything broken: one flaky test, nits only
- **Flaky test.** `crates/tmux/tests/adapter.rs` `stale_watchdog_cannot_release_a_later_adapter_transaction` **failed once** in a full suite run (`33 passed; 1 failed`). It then passed 3/3 alone and in a second full run (`34 passed`). No panic text was captured.
  - Cause (inferred): it waits for the first watchdog to fire, which takes 8 s since this PR, inside a 15 s `EVENTUALLY` while the rest of the suite runs alongside. That makes it a probable flake that the longer watchdog widened.
  - Fix: widen `EVENTUALLY` for this test, or derive it from `SUBMIT_RECOVERY_LIMIT_SECS`.
- **Nit: the hold map can leak.** `forget_holds` runs only on the worker's own ack and fail paths. A job that ends elsewhere (a first-attempt delivery, an inbox pull, an operator release) leaves its small entry in memory until restart.
- **Green at `9edfee0`:**
  - `pij-core` ports: 6
  - `pij-harnesses` lib: 194
  - `pij-daemon` lib: 653
  - `held_delivery_launches`: 1
  - `pij-tmux` adapter: 34 (second run)

## Not run
- **Not re-run:** the coder's mutant table. I spot-checked S2, which is now witnessed by `the_clear_is_decided_on_the_owned_read_not_the_look` (by reading it).
- **Not run:** clippy and fmt (the coder reports both ok).
- **Not tested live:** a Claude composer.
- **No daemon reading.**
