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
turns are not forwarded automatically. Compact/new/reload remote controls are
unsupported for Copilot; native user-driven session lifecycle remains separate.

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
`registration-wait` notices still say **retrying when available**.

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
The extension sends actual `observed_at` / `observed_seq` progress through
`/v1/inbox/heartbeat`, including while it waits for completion or operator Hold.
Timers and empty pages do not count as observations. The lease defaults to 60 seconds
(`PIJ_RS_EXT_CLAIM_LEASE_SECS`), with renewal every third of that interval. With
outstanding deliveries, frozen progress does not extend the lease horizon; three
renewal opportunities yield `native-receiver-stale`. `pij state` and `pij inbox`
show that reason separately from an active host. The daemon wakes on receiver deadlines, parks pending/unacknowledged
native bodies with `undelivered:native-receiver-unavailable`, emits
`delivery.parked` with reason `native-extension-unavailable`, and refuses later
sends with the receiver's seat named. It does not silently fall back to tmux.

A still-running extension process re-arms this on its own: a `native-receiver-stale`
heartbeat response is the one receive-hold the extension treats as retryable, so once
the daemon is reachable again (including after it restarts — a new process behind the
same address) the extension re-registers the same seat and re-establishes its
observation baseline before its next claim attempt, with no `extensions_reload` or
Copilot CLI restart needed. Every other receive-hold (malformed claims, ambiguous
sends, an unprovable discard/gap, an RPC deadline) stays terminal with a diagnostic;
those conditions are not safe to retry automatically and still require the manual
recovery below.

A correlated-anchor gap (the queryable incremental-history window not yet showing
the event the extension last observed, with no terminal proof either) is retried in
place for up to ten minutes before it escalates to that same terminal hold — a live,
busy turn can keep delivering through the push callback even while the bounded
backward-read window still lags behind it, and this window gives that case time to
resolve on its own instead of holding on the condition's first appearance. Only once
the gap outlives that window does it hold exactly as before. A hold for any reason no
longer by itself also kills heartbeat lease renewal — the two are tracked
independently, so a hold does not by itself also present as a separate
`native-receiver-stale` condition to the daemon.

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
