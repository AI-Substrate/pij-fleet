# Handover: F9 part 2 — false-positive non-retryable hold in `probeReceiver`

**Status**: diagnosed by a peer, NOT fixed, NOT tested. Needs a working session
(this doc was written from a session whose bash/subprocess tooling is dead —
do not try to continue from there; start fresh, in this `pij-fleet` checkout).

**Origin thread**: pij send between `pij-royal-viper` (diagnosing seat) and
`pij-unacceptable-behaviour` (SlideForge o-prime, tmux `%15`, originally
reported F9). Reply to `pij-unacceptable-behaviour` when this is fixed and
tested — they offered to run a patched build across their fleet and report
the HELD count back.

## Relationship to F9 part 1 (already fixed, already shipped to disk)

Part 1 fixed `native-receiver-stale` (a retryable missed-heartbeat condition):
`.copilot/extensions/pij/store.mjs` — `keepReceiverAlive()` now marks that one
hold reason `retryable: true`, `holdReceiving()` records it as `this.heldError`,
and `run()` now drives a `reconnect()` loop (~L769-810) instead of a one-shot
`consume()`, so a stale lease self-heals instead of staying wedged until a
manual `extensions_reload`. This part is live (confirmed loaded on several
seats today) and does fix that specific case.

**Part 2 is a different hold reason that part 1 does not — and should not —
cover**, because part 1 only widened the retry path for the one hold already
proven data-safe to retry. This one is currently *correctly* conservative by
the code's own design intent, but the SlideForge prime's evidence shows the
condition it's guarding against is a false positive in practice.

## The bug

`probeReceiver()` in `store.mjs`, the anchor-gap check (currently ~L1216-1224):

```js
if (completion.msgId && anchor < 0 && !completion.terminal) {
    const error = new NativeError(
        "Native receiver progress gap lacks correlated terminal evidence; receiving held without acknowledgement or reinjection",
    );
    this.holdReceiving(error);
    throw error;
}
```

`anchor` is `page.events.findIndex((event) => event.id === completion.lastEventId)`
over a **backward** read of the last 128 events (`direction: "backward", max: 128`).
If the message's pre-inject anchor event isn't in that page, this is treated as
an unrecoverable, non-retryable hold (`NativeError` defaults `retryable: false`).
`holdReceiving()` then aborts **both** `receiverController` and
`heartbeatController` (see `holdReceiving`, ~L866 — note it kills heartbeat
unconditionally, not just the receive loop), so even if this hold were made
retryable, the heartbeat dying means the daemon will separately mark
`native-receiver-stale` on its own — i.e. this bug cascades into part 1's
symptom even after part 1's fix, because part 1's reconnect only fires from
`run()`'s loop and the heartbeat side has its own independent death here.

### Why it's a false positive (SlideForge prime's evidence, 2026-10-06)

- The model **did** receive and act on the held message in every observed
  case (prime quoted msg `...0100`, consumed and acted on as a turn; `s012`
  likewise acted on `...0101`). Nothing was lost to the model — only to pij's
  own bookkeeping.
- Signature in `~/.copilot/logs/process-1791256718679-7397.log` (seat
  `pij-unacceptable-behaviour`):
  ```
  04:03:40.167Z native-accepted msg ...0100 grade native-accepted-not-model-complete
  04:03:40.931Z receive-held "progress gap lacks correlated terminal evidence" retryMs null
  (no further native-event lines at all, though the seat worked for 8+ min)
  04:12:15.308Z receiver-stopped cause SIGTERM (manual extensions_reload)
  ```
  Compare the healthy path (same log, lines 24-27): `native-accepted` →
  `user.message` native-event at **+93ms** → `inbox-acknowledged` →
  `completion-wait`. In the HELD case, the probe fires at **+764ms** and still
  finds no correlated event for this message in the 128-event backward page —
  and **no native-event for this message's `user.message` ever appears in the
  log afterward either**, even though the turn visibly ran for 8+ minutes.
- Scale (today's logs only, grep `"kind":"native-accepted"` then the next
  event): **18 HELD across 7 seats** (`pij-unacceptable-behaviour` x5,
  `pij-wandering-uldaman` x5, `pij-attitude-adjuster` x3,
  `pij-big-anglerfish` x2, `pij-anonymous-arathi` x2, `pij-disabled-marlin`
  x1); dozens OK in the same window.
- **Idle duration before accept is NOT the cause** — observed seconds-silent
  values for HELD cases: `1, 2, 4, 5 ... 638, 2727, 169151`. Spans both very
  busy and very idle seats.
- Daemon side: `~/.pij-rs/pij.sqlite` `jobs` rows 229 (a Telegram inbound from
  Vaughan, lost), 231, 235, 237, 239 — `state: failed`,
  `outcome: undelivered:native-receiver-unavailable`, `attempt: 0`, no retry
  ever attempted.

### Likely mechanism (prime's working theory, not confirmed by test)

The probe only runs after `RECEIVER_EMPTY_READ_LIMIT` (3) consecutive empty
forward reads (see the call site ~L1155, inside `readEvents`). That implies
the normal forward incremental poll (`eventLog.read({ cursor, max: 128 })` in
`readEvents`, ~L1116) is *also* not seeing the injected message's `user.message`
event during a long/busy turn — not just the backward probe page. If the
native SDK defers flushing certain events into the queryable event log until
some later point in a busy turn (rather than the extension's read cursor
being wrong), then both the forward poll and the backward anchor check would
legitimately come up empty at probe time even though the message was already
handed to the model through whatever path actually delivers it live. This
would make the "unrecoverable" conclusion premature, not wrong-but-safe.
**This needs to be confirmed against actual SDK/event-log behavior before
trusting any fix design on it** — don't assume this mechanism without
verifying.

## Constraints (read before touching this)

- This is squarely in the exactly-once / data-safety path. The surrounding
  code's own comments are explicit: most holds in this file "are genuinely
  unsafe to auto-retry (malformed claims, ambiguous sends, unprovable
  discard/gap recovery) and must stay terminal-with-diagnostic." Only one
  hold reason (`native-receiver-stale`) has so far been proven safe to
  auto-retry (part 1). Do not widen retryability here without equally solid
  proof — a wrong fix could cause a message to be silently dropped *or*
  double-delivered, which is strictly worse than the current (annoying but
  safe) manual-recovery behavior.
- Don't patch blind. Build a minimal, deliberate repro if possible (a busy
  seat receiving a `pij send` mid-turn reproduces within a few sends per the
  prime — "any busy seat receiving a pij send mid-turn reproduces it within a
  few sends; idle seats hit it too").
- Required verification before shipping anything: `just copilot-native-test`,
  `just typecheck`, `just lint`, from this `pij-fleet` checkout. Do not
  declare this fixed without real (not traced/assumed) green output from all
  three.

## Candidate fix directions (SlideForge prime's proposal, unvetted)

Offered as a starting point, explicitly "your call" — not pre-approved:

**(a) Re-check after the event callback settles.** Before declaring a gap,
wait for the `user.message` event whose content/id correlates to the injected
message (it arrives ~100ms later on the healthy path), or re-probe once after
a short delay. *Risk flagged*: needs care not to reintroduce a race — a fixed
short delay just narrows the window, it doesn't eliminate the underlying
"when does this event actually land" uncertainty.

**(b) Search forward for the correlated message instead of requiring a
backward anchor.** If the anchor isn't found in the backward page, search
forward from the inject point for the `user.message` event carrying the
injected message's id/content (proof the seat consumed it) rather than
requiring `completion.lastEventId` to be present within the last 128 events.
Looks safely additive — this only adds a second proof path, it doesn't loosen
the existing one.

**(c) Never let a receive-hold silently kill the heartbeat.** Currently
`holdReceiving()` unconditionally aborts `heartbeatController`
(~L866, immediately after aborting `receiverController`). Decouple these: a
hold on the receive side shouldn't by itself have to also tear down the
heartbeat/lease, which is what causes the `native-receiver-stale` cascade
even for holds that get resolved quickly by other means. Looks safely
additive. Needs checking against why the current code couples them in the
first place (git blame / commit history on `holdReceiving`) before assuming
it's safe to split — there may be a reason not yet visible from reading the
function alone.

(a) and (b) together would directly target the false positive at its source;
(c) limits blast radius for any hold reason, including ones that stay
legitimately non-retryable, and is probably worth doing regardless of (a)/(b).

## Where things stand

- Nothing has been coded for part 2. Part 1's fix (`reconnect()` etc.) is
  already on disk in this checkout (`.copilot/extensions/pij/store.mjs`) and
  is a separate, already-landed change — don't confuse the two when reading
  `git diff`/`git log`.
- Reply destination once this is diagnosed/fixed/tested:
  `pij-unacceptable-behaviour` (SlideForge o-prime, tmux `%15`). They are
  waiting to run a patched build across their fleet and report the HELD count
  back — that's a good acceptance test in addition to local
  `copilot-native-test`.
