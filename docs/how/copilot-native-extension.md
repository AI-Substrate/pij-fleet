# Native Copilot extension

New Copilot CLI sessions use the repo-owned native user extension for Pij body
messages. Existing sessions are not retroactively attached. No UI-server/RPC
port or tmux message injection is used. Pi/OMP/Claude transport is unchanged.

## Install and diagnose

From the **canonical main checkout**, after the native extension and Rust daemon
changes have been composed:

```sh
just link
just copilot-native-doctor
```

`just install` includes the same link policy, but reports and skips the optional
Copilot doctor when the CLI is absent. An installed but broken CLI still fails;
standalone `just copilot-native-doctor` always requires it. Production linking
refuses linked worktrees before changing any home. The destination is
`${COPILOT_HOME:-$HOME/.copilot}/extensions/pij`, linked to the reviewed
`.copilot/extensions/pij/` source, including `extension.mjs` and `store.mjs`.
`COPILOT_HOME` also selects `settings.json`.

Setup merges only `experimental: true` into a valid settings object. Unknown
keys survive; an already-enabled file is not rewritten. Foreign symlinks
(including dangling links), real destination files/directories, symlinked
configuration roots/settings, and malformed settings are refused rather than
replaced. Settings updates sync the complete temporary file before atomic rename,
then sync the parent directory where supported. Real sync errors fail setup;
an error after rename explicitly reports that changes were applied. These
preservation checks are **brakes**: removing them permits the same or more writes.

`just unlink` removes only Pij-owned extension links; it leaves experimental and
all other settings alone. The doctor is read-only and verifies the exact source
link, both source modules, and persistent experimental enablement. It does not
prove a particular running session has attached. `copilot --no-experimental`
remains a deliberate per-session opt-out, not a reason to enable a fallback.

After setup, start `copilot` manually or spawn a new Copilot seat through Pij.
The native `pij_send({to, message})` tool returns a Pij receipt. Ordinary assistant
turns are not forwarded automatically.

The extension joins with the CLI 1.0.113+ SDK shape: `pij_send` under
`tools.definitions`, and `onUserPromptSubmitted` (held FYIs, plan 158) and
`onAgentStop` (end-of-turn pij blocks, plan 226) under `hooks.handlers`. The SDK
ignores the older flat `tools: [...]` and `hooks: {...}` without an error, so a
wrong shape still registers and receives messages but loses the tool and both
hooks (F208). Copilot CLI 1.0.113 or later is required. `just copilot-sdk-contract`
checks this against the installed CLI (see [Installed SDK contract](#installed-sdk-contract-d-f208-1)).

Remote `compact` works for Copilot (plan 186): `pij send <seat> --command compact`
types `/compact` into the seat's pane through the same pane control path as
Claude's compact (the typing gate and an empty composer). It also waits for a
*settled* idle, because Copilot's busy footer reads as an empty composer: the
seat's published state must be `idle` on two registry reads 250 ms apart (Copilot
publishes `idle` for 2–4 ms between steps of one turn). The empty composer is read
from the `╻▄▄` / `┃` / `╹▀▀` box with a footer row within two rows below it. The hint
row counts as a footer when its `·` segments include `← open sidebar`,
`/ commands` and `? help`, whatever mode segments 1.0.114 puts between them, so a
pane whose footers never say `context` is still read (F210). With text typed,
1.0.114 drops those hints (`Interactive · Allow All · @ files/sessions · # issues`),
so such a pane with a draft stays unrecognized wherever the caret is. A running
activity just above the cwd row (any spinner frame `∙ ∘ ● ◉ ◎ ○` at column 0, a
space, then text, as in `◉ Compacting conversation history (1s)`) makes the frame
unreadable, never an empty composer. Copilot messages stay on this extension. `new` and
`reload` still refuse with `E-RS-CONTROL-UNSUPPORTED`; native user-driven session
lifecycle remains separate. The compact queues on `delivery-control:<seat>`, not
on the extension's `delivery:<seat>` kind, so the extension never claims it. Both
kinds share the seat's serial key, so a typed compact never overlaps a claimed
native body. Order across the two kinds is by row id among the rows due now, not
strict FIFO, in both directions. A compact queued behind pending bodies may run
before the later ones. A compact sent just before a body usually runs *after*
that body: the extension claims the body within milliseconds of its push, the
drain only ticks every few seconds, and by then the seat is `working`, so the
compact waits for that turn to end. Let a compact land before dispatching the
next packet to a Copilot seat. The signal is the `delivery.outcome` event with
`outcome: delivered` for the compact's msg_id (`pij-rs tail --json`; the msg_id
is on the send receipt), or "Compaction completed" in the seat's pane. `pij
state` lists the compact only while it is held.

`restart` is the other control pij types into a Copilot pane (F212): `pij send
<seat> --command restart` types Copilot's own `/restart`, through the same path
and brakes as `compact` (self or parent only). `/restart` swaps the CLI process
and restores the same live session. A throwaway 1.0.114 seat kept its seat id,
native session, spawn id, role holding and held FYI across two restarts; only
its pid changed. It needs the CLI started through its npm loader
(`COPILOT_LOADER_PID`, which every pij-spawned seat has); otherwise Copilot prints
"Restart is not available". Every other harness refuses `restart`.

A lost login is recovered the same way. Copilot's CLI can start answering
`session.error` `authorization` 401, or `Execution failed: GitHub credential
authority error: …` (no code), and keeps failing until it is restarted; a fresh
process re-links the login with no one signing in. The extension recognizes
both forms (by `errorType` and `statusCode`; the second by its message prefix)
and, once the failed run ends and its idle is published, posts
`{seat, native_session, pid, proc_start, kind}` to `/v1/seat/auth-error`: no
error text, and a report from a process `/restart` already replaced is refused.
The daemon then queues one `restart` when the seat is idle with an empty
composer, and otherwise tells its live parent and the designated primes of its
projects how to restart it. One restart per seat per 30 min: while that restart
is still queued, nothing more happens; once it has run, a report from the same
process means the restart did not take (they are told to `/exit`, then
`pij-rs revive <seat>`), and one from the new process means the login needs renewing
(`/login`). Each decision that acts is a `seat.auth_recovery` spine event.

One loss the extension cannot report at all (F261). Eight hours after a
Copilot process starts, its enterprise sign-in lapses: Copilot flips its
managed-extension policy from Allowed to Pending and force-kills this extension
(its host log ends `=== force-killed ===`), on an idle seat, so no run fails and
no `session.error` is emitted. The daemon sees only the lease stop renewing. Its
`native.receiver.lapsed` edge runs the same decision with kind `extension-lost`,
after reading in-process that the seat's recorded process (pid and start) is
still running: then the same brakes and the shared 30 min window apply, and a
fresh process brings the extension back. A process that is gone is never
restarted; its parent and primes are told to `pij-rs revive <seat>`. A lapse
its receiver renews before it is decided is spared, and one told before the
daemon's follower attached (a lapse a restart restores is not told again) or
lost to a lagging subscription is decided when the follower reconciles the
lapses outstanding on the spine, at attachment and after any gap.

New manual sessions receive an ordinary memorable Pij ID from the daemon, just
like spawned seats. The native session ID remains the internal resume key.
Existing addresses, including earlier hash-style IDs, are preserved; this does
not rename live seats or move their queued messages and acceptance journals.

Copilot may register a temporary startup conversation before opening an exact
saved session. The daemon can retire that same-host bootstrap registration and
reuse the saved session's existing address only after independently proving its
old attested process is dead. A live old owner, changed native session, or
unrelated bootstrap process/pane still refuses the handoff. The saved address,
parent, operator state, and queued messages are retained.

### HydraFusion models (plan 169)

`hydrafusion` (policy `standard`) and `hydrafusion-max` (policy `max`) are
Copilot's per-turn model router. Spawn them by model id:

```sh
pij-rs spawn --harness copilot --model hydrafusion-max --cwd "$PWD"
```

Pij launches `copilot --yolo --no-ask-user --model hydrafusion-max --context default`. Copilot
refuses every other context tier for these models ("HydraFusion models do not
support context tiers"), and omitting `--context` would inherit a persisted
`long_context` tier, so the explicit `default` tier is required. Copilot also
refuses reasoning effort for them, so `--effort` with a HydraFusion model is
refused before launch: `HydraFusion routes effort itself; omit --effort`.
`--model auto --auto-tier hydrafusion` is refused by Copilot 1.0.97 ("requires
dynamic Auto tiers"). `pij state` reports the seat's `boundModel` as the
synthetic id, and its `session:` line reads `model hydrafusion-max · last call
<routed model>`: the reported model is only the last call's. The cold-wake guard
still refuses a large, idle fusion seat, but it never prices the wake: the
router picks the model per call, so the price reads `? (not estimable: router,
per-call models)` rather than a number for one model.

Each turn is routed to a pattern: `single`, `cascade` (primary, review-scope
judge, conditional repair) or `critique`. On Copilot 1.0.97 the live stream of a
fused turn has this shape:

- `session.fusion_route_started` and `session.fusion_resolved` (with `fusionId`)
  arrive just before the turn's `user.message`.
- Phase output streams as ephemeral root `assistant.message` events stamped
  `data.fusion` without `commitId`. No root `assistant.turn_start`/`turn_end`
  fires before the commit, and a review-scope judge emits no root message or
  `user.message`.
- The commit replays the winning phase as ordinary root turns whose messages
  carry `data.fusion.commitId`, then `session.fusion_completed`.
- A repair phase and its commit carry a new `interactionId`, and phase parents
  are persisted-only `session.usage_record` events never seen live.

So the receiver never infers completion from events. Their order across the live
push channel and history reads, which race and can each miss events, and which
turn an event belongs to, were reconstructed through five review rounds and
each reconstruction left a gap. Copilot's run state decides instead:

- **Consumption:** the owned message's own `user.message`, matched by
  `messageId` on either channel.
- **Question:** after consumption, any root `session.idle`, `assistant.idle`,
  `abort`, `session.error`, `assistant.turn_end` or `session.fusion_completed`
  triggers one question, `metadata.isProcessing()` plus
  `queue.pendingItems()`. Without a trigger the question is still asked every
  30 s (`RUN_STATE_REASK_MS`), so a lost event costs a delay, never a stuck
  inbox. A bounded history window that shows a trigger is settled by the same
  question instead of a receiver-gap hold.
- **Completion:** the session is not processing, nothing is steering, no turn
  prompt is withdrawable, and the message is not queued. The trigger only names
  the reported completion event.
- **Failure:** missing, failing or malformed run-state APIs hold receiving with
  a named error. Completion is never guessed.

The plan-169 canary (Copilot 1.0.97-1, 250 ms polls) found `isProcessing` true
through every phase gap of a cascade: primary, judge `reject`, handoff, repair
and commit. It turned false only after `assistant.idle`. A steering message
(`delivery: "steering"`) and an abort stayed inside the run, and an abort was
followed by `assistant.idle`, `session.idle` and then not-processing. A queued
prompt started straight after the first run, so processing never fell between
them: the owned message waits for the queued run too. That over-wait is safe;
early completion is not. Uncommitted phase output is never the final message,
and the next queued Pij message waits.

The seat publishes `working` from `session.fusion_route_started`, because the
phases emit no root turn start, and `idle` at `session.idle`, `abort` or
`session.fusion_completed`. Never at a root `assistant.turn_end` (F207): Copilot
emits one at every tool boundary and the next `assistant.turn_start` ms later,
so it is not the end of the turn. A failed route stays `working`: its
fallback model still runs the turn. The `critique` pattern was not observed in
the plan-169 canary; synthetic draft → critic → revision replay passes.

### Several sessions in one Copilot process (plan 168)

Copilot 1.0.96 keeps every session's extension host alive inside one process.
`copilot --resume` loads a blank boot session before the picked one, and `/new`
leaves the old chat's host heartbeating, and neither host ever gets
`session.shutdown`. The holder of the process's seat is therefore decided by
observed history, never by registration order:

- Each extension reports `native_precedence` on `/v1/register` and on every
  receiver heartbeat. `last_user_message_at` is the timestamp of the newest
  main-agent `user.message` event in its own event log (a typed or pij-delivered
  prompt; sub-agent events never count), or `null` when there is none.
  `loaded_at` is the extension host's process start, so re-registering never
  counts as a later load. `null` means the history was read and holds no
  prompt. A history the extension could not read (the filtered read and the
  plain fallback both failed, or timed out) is sent as `history_unknown: true`,
  which ranks lowest like a blank session: it never takes the seat from a live
  holder with history, it does take over a lapsed lease, and its first typed
  prompt sets a real value. A resumed chat whose read failed therefore waits for
  its first prompt.
- The newest `user.message` holds the seat. A session with none never takes it
  from a live holder that has one. A tie (neither has one, e.g. resuming an empty
  chat) goes to the later-loaded host.
- A claim that does not outrank the live holder gets HTTP 409
  `details:{retryable:true,hold:"native-holder"}`. The extension stands by
  quietly: one stderr line, no timeline banner, no tombstone of the holder. It
  retries every 30 seconds, and at once when its own session sees a prompt
  (including one that arrived while the refused registration was in flight) or
  is resumed. After three refusals in a row it stops the 30-second retry (F162)
  and waits for that prompt or resume, checking once every 10 minutes for a
  holder that went silent. The daemon refuses such a claim before reading any
  process, so a standing-by session launches nothing there.
- It takes over without outranking only once the holder's receiver lease lapses,
  i.e. its extension stopped heartbeating (a stopped standby notices within its
  10-minute check).
- Resume without typing: the resumed chat has older prompts and the blank boot
  session has none, so the resumed chat holds the seat as soon as it loads.
- **`/new`: the old chat keeps the seat until the first prompt is typed in the
  new chat.** Messages sent in between are delivered to the old chat. Typing in
  the old chat again later moves the seat back, because it is newer activity.
- An extension that does not report the field keeps the earlier behaviour: the
  newest registrant supersedes. After a daemon restart, a holder whose report has
  not arrived yet keeps the seat until its first heartbeat reports or its lease
  lapses.

The pane's identity follows the holder, so `pij whoami --pane` and the managed
Copilot status line name the holder. The status line re-runs every
10 seconds (`statusLine.refreshInterval`, set by the daemon), because Copilot
otherwise re-runs it only on its own events and kept showing the phantom.
Sessions already running when the daemon writes that setting pick it up on
their next launch.

A `/clear` (or `/new`) in a seat pij spawned hands that seat's work to the new
session's seat (F190). Its role holdings and held FYIs move. Its delivered,
unanswered role requests are delivered again, so the cleared session's
successor reads them. The old seat names its successor in `pij directory`.
A seat with no launch id (one not started by `pij spawn`) carries nothing.
After `/clear`, the seat's roles, queued requests and held FYIs move to the new
session on its **first prompt**; until then the old id keeps them, because
plan 168's blank-session brake holds a session with no prompt behind a live
holder with history (`registration.refused`, `holder-current`).
Details and recovery: `docs/how/pij-rs-api.md` § Native Copilot registration.

## On a Copilot update (D-F208-2)

A new Copilot CLI can change its SDK without a word, as 1.0.113 did (F208): every
seat that reloads onto it loses `pij_send` and its hooks. Check before any seat
reloads.

**Take updates with `just update-copilot`** (the whole job, with when seats reload, is procedure PROC-release-0001: `just proc PROC-release-0001 --key copilot-<version>`). It runs `copilot update`, then the
[installed SDK contract](#installed-sdk-contract-d-f208-1) on the version now
installed, and:

- on a pass, says the fleet may reload Copilot seats. The contract's full pass
  has recorded a local pass for that version (below); there is nothing to commit;
- on any failure, exits 1 with `do not reload: the SDK would drop pij_send/hooks`
  and the failing checks: leave seats on the old process and fix the extension
  for the new shape first;
- with no update available, says so and still runs the contract.

**Seats never update themselves (F271c).** Every Copilot seat pij-rs spawns or
revives starts with `COPILOT_AUTO_UPDATE=false` in its `env` prefix. Copilot then
runs the version built into the installed binary, the one `copilot --version`
reports and the contract checks, ignores any newer package another Copilot
process downloaded into the shared cache (`~/.copilot/pkg`), downloads nothing,
and skips its session-start plugin updates. A seat's version changes only when
the installed binary does. `copilot update` says it downloads into the package
cache and that the download "applies at your next launch, as long as auto-update
is still enabled then", so it may not move seats forward on its own; how the
installed binary itself is replaced is not yet proven (F271c). Only launches pij
builds carry the variable: a spawn or revive `SpawnPlan`. A Copilot seat already
running when this shipped, an adopted seat pij never launched, and a seat
restarted inside Copilot (`/restart` is not a new `SpawnPlan`) are not
retrofitted; they keep auto-update on until pij spawns or revives them.

**Copilot outside pij still updates itself.** A Copilot you start by hand (the
prime's, say) checks for updates at startup and applies a download at its next
launch, and 17 Nov's 1.0.113 arrived that way. Whether that self-update also
replaces the installed binary, and so the version the next seat launch gets,
is unproven. The first sign is the contract's version warning in `just doctor`
and `harness checks` (`copilot-sdk`): `Copilot is <new>, but neither the
committed floor (<old>) nor a local pass names it: run just
copilot-sdk-contract before reloading seats.` Treat it as the same step: no more
seat reloads until the contract has passed.

**Local passes (F250).** Every full pass of the contract on the installed CLI's
own SDK writes `<state-dir>/copilot-sdk-pass/<version>.json` (pij-rs's state
dir: `PIJ_RS_STATE_DIR`, else `~/.pij-rs`; mode 0600, written to a new file and
renamed into place) with the version, the pass time, the check count and the
contract's commit (read from the checkout's git files, no launch). The contract
then accepts the installed version when the committed floor
`harness/copilot-sdk-verified.json` or a local pass names it exactly, so Copilot's
own updates need no record PR. A local pass counts only if it is whole: it names
the version asked for, has a parseable pass time and the contract's current check
count, and is a regular file (never a symlink). Anything else is treated as
absent, and the sensor warns as before. A run given `--sdk` or
`--copilot-version` checked something other than the installed CLI and records
nothing. The committed floor still moves only with `--record`, and CI, which has
no Copilot CLI, skips as before.

Cost: on demand only. No loop, hook or timer; one run is the update plus one
contract run. A local pass adds no launch: the contract writes and reads one
small file in-process.

## In-place CLI upgrade

On Linux, replacing the installed Copilot executable can leave a live host's
`/proc/<pid>/exe` link ending in ` (deleted)`. Pij accepts that replaced executable
for safe same-session reattachment, retaining the replacement fact while checking
the executable basename, argv, ancestry, exact pane and process incarnation.
The existing seat and native session are retained; messages are not replayed.
A binary literally named `copilot (deleted)` normalizes to `copilot` and reports
`replaced=true`; it is accepted under the same basename rule as any other `copilot`
executable, with argv and entrypoint checks unchanged, so the collision widens nothing.

`extensions_reload` reloads the extension, never re-execs the Copilot host.
Loader readiness therefore does not prove Pij registration succeeded. A terminal
`extension-unavailable` banner says **no retry is scheduled**, includes a safe
registration diagnostic, and directs you to restart the Copilot CLI (`/restart`
or relaunch). Do that when the banner requests it; restarting the Pij daemon does
not change the host executable. Ordinary Copilot remains usable. Transient
`registration-wait` notices still say **retrying when available**. When a
`[pij native] unavailable: …` banner was shown and registration then succeeds,
one `[pij native] reconnected` line follows, so a recovered outage never reads
as a live one. Nothing is shown when no banner was, nor for a quiet standby.
An outage the receive loop saw ends only on positive evidence from the daemon:
a claim it returned, empty or not. An idle seat's claim used to be held open
until the extension's own 65 s request timeout, so nothing ever returned, and
the banner stayed up after the outage was over (viper and seven more seats, an
hour after the 19 Nov 23:23Z bounce). The daemon now answers an idle native
claim with an empty page at 55 s (`NATIVE_IDLE_ANSWER`, under the extension's
timeout; only the idle wait is cut, never a claim in flight), and that answer
shows `reconnected`. The extension's own timeout, and a successful
re-registration, end nothing: a refusal can be late, and a daemon that registers
the seat but refuses every claim keeps one banner up rather than flapping
`unavailable`/`reconnected` on each retry. A later failure starts a new outage
and says so again.

## Immediate peer-message delivery

Peer messages use native `session.send({ prompt, mode: "immediate" })`: they
request steering during an active turn and start normally when idle. This is a
delivery-semantics change, not proof that every Copilot client queue race is fixed.
Native message-ID correlation, durable acceptance, consumption/completion grades,
duplicate suppression, consent protection and the one-in-flight barrier are unchanged.

To activate the change, a recipient must reload its native extension or start a
fresh Copilot CLI session. No daemon rebuild or restart is required. Already-accepted
queued items are neither converted nor replayed by this change.

## Receipt grades and failures

A daemon `reader-read` receipt means the native harness **consumed** the matching
message, proven by `user.message.data.messageId`; native acceptance alone is not
acknowledged. This is **not** model completion. Public native events carry distinct
event `id` and `data.messageId`. Accepted duplicates recover consumption evidence
from native history without re-injection. A crash across native acceptance and
durable recording can remain ambiguous; this is not universal exactly-once delivery.

The receiver serializes inbox messages until an owned native terminal event:
correlated `session.idle` / `session.error` / foreground `abort`, or a final foreground
`assistant.message` with no tool requests followed directly by `assistant.turn_end`.
Unrelated interactions, subagent events, global abort, and an idle-looking pane
cannot supply consumption or completion. Supported `session.rpc.eventLog.tail()`
and `.read()` APIs recover missing callbacks in cursor-based pages of at most 128
durable events, backing off from 250ms to 5s only while proof is outstanding.
No full `getEvents()` history is loaded. Missing APIs, unreadable pages or expired
cursors hold receiving without an unproven ACK or reinjection.
A correlated abort closes the consumed message's turn, not successful inference.
Picker resubmission with a different native message ID never grants another Pij ACK.

If an interrupt removes an accepted envelope before consumption, recovery waits
for a fresh post-send turn/idle boundary. It requires all three facts: fresh
history lacks the message, the supported native queue/steering snapshot no longer
contains it, and no potentially consuming turn is processing. A queued item awaiting
a later run is a **no-duplicate brake**, never a reason to resend. Recovery retains
the same unacknowledged daemon claim, durably reserves an exclusive retry intent
keyed by the old native ID, then re-injects once and waits for new consumption.
The timeline reports `📨 re-queued 1 pij messages after interrupt`. Unsupported
or ambiguous recovery proof holds safely rather than guessing or fabricating a
human-typing hold. A crash during native submission can still leave an ambiguous intent.

The extension claims with native-session/host-incarnation evidence and checks the
daemon's echoed proof before injecting. Missing support cannot silently downgrade
to an old daemon, shim, or tmux path. Human draft/typing is a consent brake: held
work stays durable, then delivers after release without another sender message.
A malformed or ambiguous claim stops affected receiving with a diagnostic, not a
successful ack or a replay loop. Daemon/key absence must leave ordinary Copilot
usable with a bounded reconnect and one actionable diagnostic per outage episode.

Copilot typing protection is draft-aware: a recognized blank composer permits
delivery immediately, even just after submitting or clearing a prompt. Recent
edits delay delivery only while a nonblank draft remains. An unchanged draft may
still age out under the configured typing grace; explicit operator Hold remains
a separate veto. Missing or unrecognized composer observations do not count as blank.

Post-join outages expose `[pij native] unavailable:` through the native session,
once per failure episode. The smoke counts the terminal prefix and proves an
ordinary user/assistant interaction before starting its isolated daemon. A
pre-join SDK/import failure can instead emit extension stderr or a native loader
error; that is not proof of timeline visibility. No private acceptance-file or
undocumented log-directory layout is required by the smoke.

### Receiver memory and emergency manual pull

The native receiver snapshots a tail cursor before each new message. Recovery of
an accepted journal entry walks bounded pages, retains only that message's
correlation metadata, and never treats old historical boundaries as fresh discard
proof. Pending observation races do not retain history bodies or accumulate
listeners for the lifetime of the session.
At child startup, registration preserves the outgoing identity, then one bounded
SDK history window establishes actual observation before the first receiver
heartbeat or inbox claim. A recycled child cannot immediately hold itself on a
stale lease merely because its local counters started at zero. Startup observation
failure holds receiving but leaves the explicitly registered outgoing tool usable.

Every awaited receiver SDK call (`eventLog.tail/read`, `queue.pendingItems`,
`metadata.isProcessing`, `native.send`) has a 15-second deadline. A miss logs
`receive-held` with the call name; ambiguous sends retain their durable intent.
After three empty reads, a tail probe (at least one second apart) detects session
movement independently of callbacks, including while the inbox is idle. Recovery
reads a bounded chronological backward window, then continues only from a forward
tail cursor. Missing overlap/correlated terminal proof holds without ACK or resend.
Shutdown with outstanding work logs `receiver-stopped` and its job/message identity.

Receiver presence is separate from the Copilot host's process and Working state.
The extension renews `/v1/inbox/heartbeat` every third of the lease, including
while it waits for completion or operator Hold; the lease defaults to 60 seconds
(`PIJ_RS_EXT_CLAIM_LEASE_SECS`). Lease liveness means heartbeats arrive, nothing
more: a busy turn with no new native event (one long tool call) keeps its lease and
its queued mail is delivered when the turn ends (plan 167). `observed_at` /
`observed_seq` are still sent but are diagnostics only. If heartbeats stop, the
daemon wakes on the receiver deadline, parks pending/unacknowledged native bodies
with `undelivered:native-receiver-unavailable`, emits `delivery.parked` with reason
`native-extension-unavailable`, and **holds** them and later sends for up to two
hours from each send (F260): the renewal returns them to the queue, once, and the
sender is told by `pij-bg` that they are held, and again if the hold ends first.
`pij state` and `pij inbox`
show `native-extension-unavailable` separately from an active host. It does not
silently fall back to tmux.

A lease answer never holds the extension permanently: a `stale` or `unavailable`
answer from an older daemon, a refused or malformed renewal all re-register and
keep renewing, replaying nothing as progress. Turn-state publication (`working` /
`idle`) continues while receiving is held, so a held seat never reads `working`
while idle. Registration reports `extension_build` and `extension_path` the way OMP
does, so `pij state` names the loaded build.

A correlated-anchor gap (the queryable incremental-history window not yet showing
the event the extension last observed, with no terminal proof either) is retried in
place for up to ten minutes before it escalates to a terminal receive-hold — a live,
busy turn can keep delivering through the push callback even while the bounded
backward-read window still lags behind it, and this window gives that case time to
resolve on its own instead of holding on the condition's first appearance. Only once
the gap outlives that window does it hold exactly as before. The retry is not a hold:
heartbeat renewal continues, as during any long turn. Every integrity hold, including
the escalated gap hold, still stops renewal, so the daemon's lease expiry applies.

If only the extension died, run `pij inbox --json` inside the still-live Copilot
seat's shell. The normal pane/session identity ladder authorizes this explicit
pull and ACK; a different native session refuses. A live receiver lease refuses
manual pull with `details.retryable:true` and `details.expires_in_ms`, even if the
host looks idle or is on Hold. After extension death, wait up to the remaining
lease (60 seconds by default); already-claimed mail is not manually reachable
during that window. This protects against competing delivery, not lost mail.
Once the lease expires or is absent, clear operator Hold and retry. Pending and
running native bodies are first parked for receiver unavailability, then recovered
with their original job/message IDs and parking history. Other parked outcomes
are not recovered. Manual reads never renew extension presence. Paneless consumers
still require the exact registered native-session/host tuple.

Deploy memory and receiver-lease support together: build/restart the composed
daemon using the operator-owned lifecycle, then reload/restart affected native
extensions. No live daemon or global extension link is changed by the isolated
proof below.

```sh
just copilot-native-memory-smoke --copilot-bin <official-copilot-launcher> \
  --copilot-runtime-dir <installed-current-platform-package> \
  --message-interval-seconds 1
```

This copies only the specified runtime code into a private cache and pins its
version; it never copies authentication or touches the global cache. The proof
uses a real Copilot host/extension and local Rust daemon, a deterministic local
model, at least 60 MiB/20,000 historical events, 30 delivered messages, ten-second
receiver RSS samples, extension-only termination, and actual native-shell CLI
pull. Receipts live under plan 150's evidence directory; private runtime/state
files are ignored by Git.

Normal startup typing-sensor readiness waits are recorded in extension diagnostics,
not shown as timeline outages. Inbox acquisition still waits for an available
sensor; actual daemon errors and stopped receiving remain visible.

## Deterministic checks

```sh
export VITEST_MAX_WORKERS=4 VITEST_MAX_THREADS=4 CARGO_BUILD_JOBS=4
just test harness/scripts/link-global.test.ts harness/scripts/copilot-native-smoke.test.ts
just copilot-native-test
just copilot-native-rust-test
```

The first command covers installer safety and proof-fixture boundaries. The
second runs the native extension's `node:test` files and is included in both
`harness checks` and `just self-check`; missing test files fail, never skip. The
Rust recipe runs `pij-core`, `pij-store`, `pij-harnesses`, `pij-daemon`, and `pij-cli`
with a checkout-local Cargo target. During parallel implementation use only the
assigned scoped checks; the composed native recipes and full gate run afterward.

### Opt-in cross-language contract

```sh
just copilot-native-contract-test
```

This explicitly runs the daemon's ignored `native_runtime_cold_resume` integration
target against the actual JavaScript store module, HTTP, and SQLite. It requires
Node, Cargo, the composed daemon test target, and `.copilot/extensions/pij/store.mjs`.
Missing prerequisites fail the named invocation; nothing is skipped or stubbed.
Node checks the module's syntax before Cargo runs. The recipe sets
`PIJ_NATIVE_RUNTIME_MODULE` to that checkout's absolute store path,
`CARGO_TARGET_DIR` to its local `target/`, and all three worker/build caps to `4`.
Cargo runs with `--locked` and `--ignored --nocapture --test-threads=1`.
`--locked` protects the dependency lockfile, not the Rust toolchain version;
retain actual compiler/Cargo versions with proof receipts.

This is opt-in: default Rust-only tests do not require Node, and neither
`harness checks` nor `just self-check` adds this invocation. A passing contract
does not establish actual Copilot CLI attachment or real-provider inference;
those still require the live witnesses below.

### Installed SDK contract (D-F208-1)

```sh
just copilot-sdk-contract            # --json for one report, --record after a new version passes
```

The fake SDK in `extension.test.mjs` mirrors the real one by hand, and F208 is
what happens when it stops mirroring it. This check uses no fake. It runs the real
`extension.mjs` in a child process with `@microsoft/copilot-sdk/extension`
resolved to the installed CLI's own `~/.copilot/pkg/<platform>-<arch>/<version>/copilot-sdk/extension.js`
(the version `copilot --version` reports), and plays the CLI's side of that SDK's
JSON-RPC wire on the child's stdio (`connect`, `session.resume`, `hooks.invoke`,
`session.event`). It passes when, through the SDK's own lookups:

- the SDK declares `pij_send` (with a handler) and hooks with handlers in
  `session.resume`;
- `hooks.invoke` for `userPromptSubmitted` and `agentStop` reaches pij's
  `onUserPromptSubmitted` and `onAgentStop`;
- an `external_tool.requested` for `pij_send` reaches pij's handler and its answer
  comes back as `session.tools.handlePendingToolCall`;
- `assistant.message` and `session.idle` events reach pij's `session.on`
  listeners (the run's end and the idle publication ride these).

A shim around the real `joinSession` wraps each function in the config where it
sits, so a call names the handler that ran; it never changes the shape the SDK
reads. The child's daemon address is unreachable, its state dir empty and it has
no `TMUX_PANE`, so no handler can do more than answer "not ready". No daemon,
pane or Copilot process is touched.

The calls are reported on the child's stderr, one line each, while answers come
back on stdout, and the two pipes have no order between them. So after each hook
or tool answer the check sends a `contract.barrier` event and waits for the
shim's barrier line on stderr: every call reported before it has arrived by then.
It reads stderr by line, never by chunk. `copilot-sdk-contract.test.ts` replays
the run with each stderr event 50 ms late and cut mid-line (R259-001).

It skips (exit 0) when no Copilot CLI is installed, as in CI, so it guards the
machines that run seats. It runs in `just doctor` (a failure is reported there,
as every doctor line is), `just self-check`, and `harness checks` as the `copilot-sdk`
sensor. A pass is `pass` when `harness/copilot-sdk-verified.json` (the committed
floor) or a local pass names the installed version (§ On a Copilot update), and
`warn` otherwise. Any failure fails, and the `copilot-sdk` sensor takes only an
explicit `pass`, `warn` or `skipped` from the report (anything else is a `warn`
carrying the output). Pass `--copilot-version <v>` or `--sdk <path>`
to check another installed runtime. The pre-F208 flat shape passes on 1.0.112-0
and fails on 1.0.113+, and the current shape fails on 1.0.112-0, so the check
follows the SDK, not a shape written into it.

Cost: on demand only, never on a hot path. One run launches `copilot --version`,
one `node` child and the extension's own two `ps` probes, in about a second.

## Real CLI witnesses

Build the composed Rust binary, then run the actual client driver:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build -p pij-cli --bin pij-rs
just copilot-native-smoke --mode both --provider local --output /tmp/native-local-receipt.json
just copilot-native-smoke --mode both --provider real --model <verified-model> --output /tmp/native-real-receipt.json
```

`--mode manual|spawned|both` selects direct Copilot launch or actual Rust Pij
spawn/prebind. `--provider local` is a **deterministic OpenAI-compatible fixture**,
not real inference. Real mode requires an inherited `COPILOT_GITHUB_TOKEN`,
`GH_TOKEN`, or `GITHUB_TOKEN` plus an explicit model. Supply credentials in memory;
never place tokens in command arguments, receipts, or repository files. The driver
never copies global authentication files and redacts known credentials from its
receipt. Signed-in `gh` alone does not populate an isolated HOME.

The driver creates private HOME/COPILOT_HOME/state, an empty workspace, a dedicated
tmux socket/config, and an owned real Rust daemon. Manual children have no Pij
identity/preallocation or SDK attachment variables; only the isolated daemon
endpoint/state overrides are present. No machine daemon restart, global install,
upstream binary update, or live-fleet call occurs. An isolated executable wrapper
adds safe Copilot launch flags for the spawned case; it execs the actual CLI and
preserves the host process identity.
Only the canonical path of that empty temporary workspace is seeded in the private
native `config.json`'s `trustedFolders`, not `settings.json`.
Global trust and tool permissions are unchanged; this is not `--yolo` or allow-all.
`--workspace-trust prompt` omits that seed: this negative control should time out
at `Confirm folder trust` before any provider request. Compare it with the default
`--workspace-trust seeded` using separate full-driver invocations, which create
distinct fresh HOMEs. A new session in a previously approved HOME is not a valid
control: native remembered trust survives there. `verifyWorkspaceTrustDiscriminator`
checks the two receipts, the pre-launch config, exact prompt path, absent provider
activity in the negative control, and correlated ordinary native interaction in
the seeded run. Passing this startup discriminator is not full transport acceptance.

The manual reply step simulates a human approving **one** native extension-tool call.
Before sending Enter, it requires the active bottom modal's exact
`Run extension tool "pij_send"` title, parses that modal's own JSON arguments,
and compares the complete object with the expected isolated recipient and nonce.
It rechecks the dialog and requires `1. Yes` to be selected; session-wide approval
is never chosen. Complete unknown/mismatching active dialogs fail without confirmation.
Welcome/transcript boxes above later UI are not active dialogs; an incomplete
repaint waits boundedly, never authorizes, and cannot prove post-key dismissal.
The complete known Copilot welcome layout is also waiting when its closing border
is bottom-most before the composer paints; altered or unknown complete boxes
still fail. This narrow exception never authorizes or proves dismissal.
The same discrimination keeps the normal composer usable beneath welcome panels. Receipts
retain before/rechecked/after terminal evidence and the exact decision, alongside
the existing final-pane captures. This is test-human consent, not a permission
bypass or tmux message-body delivery; product policy/default grants are unchanged.

The spawned reply step can instead observe execution under Pij's **existing**
launch permissions; it adds no grants and sends no consent keys. It retains the
actual process/seat and requires native permission state enabled **before** the
exact `pij_send` invocation, a matching successful tool-call ID/result, and the
result's message ID joined to the durable outgoing job with exact sender,
recipient, and body. A later permission grant cannot authorize earlier execution.
Missing dialogs or `PIJ_NATIVE_DONE` text alone prove nothing; active mismatched
dialogs fail and incomplete frames wait without keys. The receipt labels this
`native-tool-preauthorized-execution`, never human approval or model completion.

AC6 types one fixture-human draft, then sends the message only through the daemon.
The driver requires the exact native-owner echo from `/v1/inbox/typing`, a positive
daemon-reported typing grace, and a durable `delivery.held` receipt with no native
acceptance or ACK. The **same nonempty draft stays unchanged** while recency expires
and `delivery.released` precedes the original job's `delivery.inbox-ack`. Exactly
one native user message and one `reader-read` row are required; release alone is
not delivery. Missing/unavailable observations fail explicitly. No second send,
driver-issued release, or draft clear can satisfy this proof. Only after it passes
does a separately recorded human-control cleanup clear the fixture draft so the
existing lifecycle scenarios can continue. Allow enough timeout for the observed
grace; scoped assertion tests are not live AC6 or lifecycle acceptance.

Each typing sample (including failed `last_observation`) and final pane capture
also retains `pane_capture`: actual tmux `cursor_x`/`cursor_y`, `pane_width`/
`pane_height`, timestamp, raw metadata, and the paired **unjoined viewport** in
`terminal`. Coordinates are zero-based viewport positions, not offsets into the
existing joined scrollback. Metadata and screen come from one tmux command batch;
this nearby harness capture is not an atomic snapshot of the daemon's earlier
sensor read. Capture errors remain explicit, never inferred coordinates or quiet
typing. This evidence does not change composer recognition or hold/release policy.

Lifecycle controls retain `staged_pane_capture` and `before_submit_pane_capture`.
The driver binds the typed command to the measured cursor in the actual composer
row. Copilot's known `/new [prompt]` and `/exit [print]` ghosts are accepted only
with the cursor directly after the literal command and exactly one matching
adjacent selection. `/exit print` is a distinct, refused alternative; neither
placeholder is typed. Changed cursor, suffix, selection, or capture failure prevents
Enter; the driver rechecks after the staged hook and immediately before submission.
This is lifecycle fixture validation, not a product recognizer change. Retained
frame/unit tests do not establish a successful native rollover.

Each missing-consumer control gets a fresh private `HOME`, `COPILOT_HOME`, and
`XDG_CONFIG_HOME`, including the controls within manual and spawned modes. The
fixture reuses managed extension installation and the run's workspace-trust setup,
then passes only those three home overrides to the negative tmux window. The
private daemon, workspace and provider stay shared; primary settings and tmux
server environment are not modified. This isolates the observed persistence of
`experimental:false` by `--no-experimental`. Receipts retain negative launch flags
and settings plus primary settings before/after; primary `experimental` must still
be exactly `true` after the negative launch and immediately before cold resume.
The driver fails on contamination rather than repairing it to make the proof pass.

Cold-host resume treats `/exit` submission separately from process death: in the
observed multi-session CLI it closed the foreground conversation and restored an
older one while the host survived. The fixture then records the original process
tuple, verifies launch ownership and its dedicated tmux socket, captures the
remaining owned pane, and closes only that pane. An already-absent pane requires
no close; a disappeared pane alone never proves host death. Ownership checks are
one-directional safety brakes, not lifecycle or retention policy. Only actual
`kill(originalPid, 0)` returning `ESRCH` permits the existing fresh manual
`--resume` path; permission errors, a surviving process, and failed teardown do
not. Receipts retain native control, teardown, and host-death evidence separately.
Same native/Pij identity, a new process tuple, and subsequent delivery still need
the resumed live witness; scoped helper tests do not prove those outcomes.

Witnesses retain exact source and executable hashes, versions, processes/panes,
Pij msg/job IDs, public native events, bidirectional tool-originated replies,
initial spawned task/duplicate checks, held/admitted draft evidence, missing
consumer refusal/queue evidence, and daemon restart/key rotation. Local-provider
protocol proof alone is not composed product acceptance. A passing full inventory
also does not replace actual manual/spawned and real-model witnesses.
Before teardown, the receipt captures each remaining pane's terminal text and
scrollback, including on failure. An individual pane's capture error is retained
without discarding the other panes; a listing error is also explicit.

Use `--pij-bin`, `--copilot-bin`, `--timeout-seconds`, or `--output` to select a
composed build and bound a run. Existing receipts are never overwritten. Exit
codes: `0` passed, `1` failed assertion/runtime, `2` missing prerequisite or invalid
invocation. Missing binary/shipping module/real credential names the prerequisite
rather than manufacturing a fixture success. Owned processes are stopped; private
run directories remain for diagnosis and contain native session state. Remove
only the specific run directory after retaining the needed redacted receipt.
