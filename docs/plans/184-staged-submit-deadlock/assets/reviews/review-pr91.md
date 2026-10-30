# Review: PR #91 (plan 184, staged-submit deadlock) at `cff8819`

Reviewer: `pij-review-pr91`. Read-only. All scratch tests were removed afterwards (`git status` is clean at `cff8819`).

**Verdict: merge after fixes. 1 blocker.** The composer gate holds up well: no finding lets pij delete a human's non-whitespace text. The blocker is in the hold-limit policy, and it also decides what the deploy does to walrus.

## Blocker

### B1. Typing holds burn through the hold limit at zero delay, so commands fail while nobody is holding the composer
`crates/daemon/src/pointer/worker.rs:395-414` (the limit arm) and `:52-58` (`hold_delay` returns `ZERO` for anything other than composer-busy).

- The limit counts **every** `Deferred` attempt, including `human-typing`.
- `human-typing` retries on the next tick, every 5 s at the default `delivery_interval_secs`. `DEFAULT_TYPING_GRACE_MS` is **60 s**, so 12 attempts fit inside a single grace.
- The worker also ignores the gate's `next_retry_at`.
- The doc claim "Typing already waits out its grace inside the gate" (`docs/how/pij-pane-signals.md`, new section) is false: the gate returns at once.

Failure scenario: a human types into a seat. A `/compact` arrives within the grace. About a minute later it fails `undelivered:hold-limit … (human-typing)`, possibly with a blank composer. On main it waited and was delivered.

Evidence (scratch test: composer with a fresh draft, grace 60 s, 12 `drain_once` calls):
```
REVIEW retried=[(JobId(1), 0ns) ×11]
REVIEW failed=Some("undelivered:hold-limit: /compact was still held (human-typing) after 12 attempts") after 3.063417ms
```

The same mechanism decides the deploy (S5): the first look after a bounce counts as an edit, so walrus's head fails as "human-typing".

Fix options:
- keep `human-typing` out of the limit, or retry at `verdict.next_retry_at`;
- or bound the time held composer-busy rather than the number of attempts.

Also use `job.attempt + 1` in the message instead of the constant (N1).

## Should-fix

### S1. The hold-limit "tell once" dedupe never matches, so one `delivery.hold-limit` fact is written per attempt forever, and the test that claims otherwise lies
- `worker.rs:723-733` calls `latest_matching_message(&msg.from, HOLD_LIMIT_EVENT_KIND, &msg.msg_id)`.
- `SqliteSpine` matches on `json_extract(payload,'$.msg_id')` (`crates/store/src/spine.rs:220`), and the fake spine does the same.
- `hold_limit_event` writes `messageId` and has no `msg_id` (`crates/core/src/delivery.rs`).

Evidence on the **real** `SqliteQueue` (body at attempt 11, `not_before` reset between passes):
```
REVIEW real pass 0: attempt=12 hold_limit_events=1
REVIEW real pass 1: attempt=13 hold_limit_events=2
REVIEW real pass 2: attempt=14 hold_limit_events=3
REVIEW real pass 3: attempt=15 hold_limit_events=4
```
The lookup returned `found=false` on every pass.

The sender still gets one notice, because the `delivery.hold-notice` audit in `park_notice::notify` dedupes it (that audit does carry `msg_id`). The real cost is:
- one spine row per minute per held body, indefinitely;
- a notify round each minute;
- a dedupe line in the worker that does nothing.

`held_body_tells_its_sender_once_at_the_hold_limit_and_stays_queued` passes only because `FakeQueue` resets `attempt` on re-claim. After `release_deferred` the job comes back at attempt 1, 2, 3 (retried delays 60 s, 2 s, 4 s, 8 s), below the limit. So the dedupe is never exercised.

Fix:
- add `"msg_id"` to the payload, or look up `HOLD_NOTICE_KIND`;
- move the test onto `SqliteQueue`, as `failed_control_head_unblocks…` already does.

### S2. Nothing witnesses the re-read under ownership, which is the R1 ordering guarantee
`crates/harnesses/src/interaction.rs` (`submit_with_owned_input`, the clear branch).

Mutant A: decide the clear from the launch-free look (`own_leftover`) and pass `staged_leftover(text)` to the verb instead of the text captured under ownership. **It survived**:
```
cargo test -p pij-harnesses --lib            → ok. 190 passed
cargo test -p pij-daemon --lib plan184       → ok. 10 passed
cargo test -p pij-daemon --test held_delivery_launches → ok. 1 passed
(whole pij-harnesses + pij-daemon run: no FAILED)
```
Every test serves the same capture to the look and to the ownership read. The doc says "everything it lets through is judged again under ownership", and that is the safety argument. Today one refactor could delete it with every test green.

Fix: add a gate test where the look shows the exact leftover and the capture under ownership shows `leftover + "x"`. Expect `Deferred(ComposerBusy)`, no `clear_staged_leftover`, and input restored.

Mutant B, for contrast: deleting the matcher check inside the real tmux verb was **killed** by `clear_staged_leftover_refuses_a_composer_a_human_touched`.

### S3. The tolerated whitespace edits are not "cleared with the leftover": the human's whitespace is deleted and pij's text is left behind
`crates/core/src/ports.rs:759` (matcher) and `crates/tmux/src/lib.rs:~995` (`send-keys -N {chars} BSpace`).

The matcher forgives two edits that capture cannot show:
- **trailing spaces**: `capture-pane` drops them and every row is `trim_end`ed;
- **a line break typed after a space** (the admitted residual).

In both cases the real buffer is longer than N, so N backspaces from the caret stop short:
```
REVIEW newline-after-space: accepted=true backspaces=103 buffer_chars=104 residue_after_clear="/"
REVIEW trailing-spaces:     accepted=true backspaces=103 buffer_chars=106 residue_after_clear="/co"
```
(Arithmetic on the real buffer, assuming the caret is at the end, which is what the matcher sees.)

Two consequences:
- **R1 is breached, but only for whitespace keystrokes.** The human's spaces or newline are deleted.
- **The deadlock comes back.** The residue (`/` opens Claude's slash menu) is not pij's exact leftover, so the seat is held as a "human draft". A body then waits indefinitely and a command goes to the hold limit.

The doc's "It changes no characters" is wrong.

Likelihood: very low. It needs a whitespace-only edit to pij's notice, then 60 s with nothing visible changing.

Text after the caret is safe. `BSpace` deletes backwards, and `clip_multiline` hides only what follows the caret on its row, so a human edit after the caret is never deleted. A caret moved without editing leaves pij residue only.

Fix:
- re-capture under ownership after the clear and confirm the composer is blank;
- use `capture-pane -N` on this path so trailing spaces are visible;
- accept a row break as a soft wrap only where the next word could not fit in the pane width.

### S4. Bodies can still deadlock behind their own leftover when it is not the full text
R1 says "EXACTLY what it recorded at abort". The code rebuilds only `staged_leftover(full text)`.

- A body of up to `MAX_TYPED_FRAME_BYTES` (8 KiB) is 9 chunks: about 22 launches plus 80 ms of pauses. That is about 3.1 s at p90 and about 6.6 s at p99, so it can still exceed the 4 s bound.
- If the bound trips inside the loop (`lib.rs:320`), the leftover is a **prefix** plus the notice. Worse, the notice paste opens a second bracketed paste inside one that was never closed.
- That leftover is never recognized. The body stays queued indefinitely with one notice to the sender, which is #86's shape for bodies.
- Unverified: Claude may collapse a multi-line paste into `[Pasted text #N]`, which would defeat recognition for the full-text case too. I did not test a live Claude composer.

Fix: record the bytes actually pasted at abort and match against that. Or give a held body a terminal path (a pointer fallback) after the limit. If deferred, file it as a follow-up.

### S5. Deploy sequence on walrus is not safe as described
Modelled with a scratch test: head job at attempt 9180, walrus composer, fresh gate, 60 s grace, one drain:
```
REVIEW deploy head: failed=Some("undelivered:hold-limit: /compact was still held (human-typing) after 12 attempts")
REVIEW deploy calls=[]        ← no reserve, no clear
```

So, at the first tick after the bounce:
- The head fails as "human-typing" (false: it is the gate's first sight) and "after 12 attempts" (false: it was 9181). The leftover is **not** cleared.
- The next queued `/compact` waits out the grace in zero-delay ticks, and may also fail human-typing (B1).
- A later one clears the leftover (`staged-unsent`).
- The remaining rows, mostly stale `/compact` from eagle since 10-28 (diagnosis: seven rows behind the head), are then **submitted back to back** into another prime's seat.

Eagle gets several notices. R2 says hands off walrus and marmot, but the deploy itself acts on them.

Recommendation:
1. Fix B1 first.
2. Before bouncing, have eagle's prime release or cancel walrus's queued rows, or explicitly accept that the stale compactions will run.
3. Take the live cost reading only after that.

## Nits
- **N1.** The `hold-limit` detail prints the constant `HOLD_LIMIT_ATTEMPTS` rather than the real count (`worker.rs:~407`).
- **N2. Timing change.**
  - The arithmetic is sound for **one-chunk** bodies: 6 × 553 ms = 3.3 s against the 4 s bound, and the watchdog's 8 s exceeds 4 s plus 1.7 s for the abort.
  - It does not cover multi-chunk bodies (S4).
  - Cost to a human typing at the same moment: as I understand tmux, keys sent to an input-off pane are **discarded, not buffered**. In the tail, a human can lose up to 4 s of keystrokes silently, or up to 8 s if the daemon dies mid-transaction (the watchdog restores input). The doc says the human's input "stays disabled"; it should say the keys are dropped.
- **N3. Render-lag race.** A key that reached the app just before `select-pane -d` but was not yet rendered at the capture under ownership would be eaten by `BSpace ×N`. The window is tiny, needs 60 s idle first, and the existing stage path has the same race (there the key merges into the body).
- **N4. Unverified.** `fail_pane_delivery` acks the job terminal, then publishes `delivery.parked`. If the publish fails, the sweep has no fact to find, so the sender is never told. I did not check whether existing park paths use the same order.

## Checked and fine
- **TS vocabulary sync.** `rg --hidden` over `.omp .copilot crates skills docs harness` found:
  - `PARKED_OUTCOMES` (`.omp/extensions/pij/adapters/rust-runtime.ts:141`) is the only full copy, gated two-sided against the Rust enum by `rust-runtime.test.ts:216`;
  - the `peek_parked` exclusion is justified, because commands are acked `Failed`, never inbox-parked;
  - there is no TS union of deferral reasons, so `leftover-cleared` needs no sync;
  - `.copilot` has no hits.
- **Clear verb.** It runs only after `require_staged` (token plus input-off) and with `!staged.staged`, inside `if-shell` on the token. The `-e ; send-keys ; -d` sequence is one server-side command list, so no client input lands between them. A watchdog that has already taken the token turns the clear into `exit 75` (safe).
- **Matcher.** Single-character insert or delete at every index, a doubled internal space, a joined notice line, a deeper indent, other text, and paste placeholders are all rejected.
- **FIFO unblock on the real queue.** `failed_control_head_unblocks…` holds. A command that fails `hold-limit` or `staged-unsent` is acked once, and nothing is resubmitted, so nothing is duplicated.
- **Composer-busy backoff** (1 s doubling to 60 s) and zero-launch waiting (`held_delivery_launches.rs`) are as claimed. Cost-wise, S1 adds one spine write per minute per held body.
- **Baseline green at `cff8819`:**
  - `pij-core` ports: 4
  - `pij-tmux --test adapter`: 33
  - `pij-daemon` `plan184`: 7
  - `pij-daemon` `park_notice`: 11

## Not run, and why
- **No live Claude composer.** I did not test slash-menu state, paste collapse, or how Ink's backspace treats newlines. That needs a Claude canary, and the packet put live seats and tmux typing out of scope.
- **Not run:** `harness checks`, clippy, or the full workspace (CI receipt exists). I did not re-run M1–M6.
- **No live daemon reading.** I did not touch the machine daemon, not even `ping`.
