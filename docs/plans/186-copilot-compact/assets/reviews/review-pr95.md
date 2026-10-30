# Review: PR #95, plan 186 (remote compact for Copilot seats)

Reviewer: pij-review-pr95. Head `c0258a2`. Read-only. Worktree restored clean (`git status --short` is empty). My probe tests are saved outside the repo at
`$SCRATCH/probes/`.

**Verdict: merge after fixes. 0 blockers, 4 should-fix, 3 nits.**

The core design holds. The two claimers can't take each other's rows, the serial key does keep a typed compact from overlapping a native body claim, `new`/`reload` still refuse, and the change adds no launch site. The serious finding is S1: a control claim that is never acked can still wedge a seat's native bodies, through a path other than the extension claiming it. The fix is about one line.

---

## Should-fix

### S1. A control claim the drain never acks can block native bodies for 5 min, or forever if the seat comes back paneless
`crates/daemon/src/pointer/worker.rs:119-126` and `crates/store/src/queue.rs:179-215`

- **Mechanism.** An expired claim is only swept by a claim call that lists that kind (`kind IN json_each(?2)`). The native extension claims only `[delivery:<seat>]` (`delivery/mod.rs:1632`, `:559`, `:1907`). So only the drain can free an expired `delivery-control:` claim, and the drain lists that kind only when the seat has a pane or is tombstoned (`worker.rs:122`). While the control row is `running`, `CLAIM_NEXT_SQL` (`serial_key NOT IN (… state='running')`) holds back every native body for that seat.
- **Failure scenario.** The drain claims a compact, then the daemon dies before it acks or retries (a kill or crash during the submit window, which is under a second). With the pane kept, native bodies stall for the claim lease (`claim_lease_secs` = 300 s, `config.rs:199`), until a drain pass sweeps the row. If the seat comes back paneless (native paneless resume exists: `registration.rs` "native_session_resume…", `retired_paneless_copilot_*`), the drain never lists the kind and **native bodies are stuck forever**. The operator has no way out either: `release_inbox_head` → `extension_body_claim` is OMP/Pi-only (`delivery/mod.rs:2003-2011`).
- **Evidence** (probe `zz_review95_wedge.rs`, real SqliteQueue + real DrainWorker, lease 1 s, three drain passes after expiry):
  ```
  native body claim after restart, pane kept : Some("b-1")
  native body claim after restart, paneless  : None
  ```
  Store-level probe `zz_review95_probe.rs`: after the control lease expired, `claim([delivery:seat])` returned `None`. It returned `Some("b-1")` only after a `claim([delivery-control:seat])` call had swept the row.
- **Fix (in scope, no launches).** List `control_kind` for every Copilot seat in the drain inventory whatever its pane. `handle_claim` already releases a paneless claim with backoff (`worker.rs:236`), which leaves the row pending and frees the serial key. Add the paneless case as a test. To also shorten the 5-minute stall, the native claim could sweep its own seat's expired control row (fail it with "outcome unknown"). That is optional.

### S2. Two skill lines still tell agents that Copilot compact is refused
`skills/pij/references/00-routing.md:29` ("Copilot programmatic compact/new/reload are unsupported") and `:83` ("Copilot/paneless controls refuse").

- **Failure scenario.** C1 and C3 are the shared conventions most routes cite. An agent following them will never compact a Copilot worker, which is the regression this PR fixes. `peer.md` and `pa.md` were updated; these two lines were missed.
- **Evidence:** `rg --hidden -n -i copilot skills docs/how -g '*.md' | rg -i compact` lists both lines.

### S3. The busy brake can let `/compact` through mid-turn, and the staged-submit gate does not catch it
`worker.rs:257-266`

- **The gate does not catch it.** I ran the canary's Copilot 1.0.105 frame through `composer_region`. A busy frame reads exactly like an idle one:
  ```
  idle               -> Recognized("")
  busy               -> Recognized("")      (◉ Thinking above, empty ❯)
  ```
  So the published `working` state is the only guard. The brake reads it once, from the registry row that `handle_claim` loads (`worker.rs:192`).
- **How likely.** In the canary, the intra-turn blips lasted 2–4 ms, about two per 27 s of turn (…298609/…298611 and …322585/…322587). That is about 2e-4 per drain pass. A held compact makes roughly 5 passes (backoff 1, 2, 4, 8, 16 s), so about 1e-3 per compact sent mid-turn. The risk grows with tool-heavy turns, if each tool step blips.
- **What Copilot does with it.** I could not determine this statically and did not run it live (see Not run). Copilot's changelog, 1.0.79: "Queue prompts, shell commands, and supported slash commands in local sessions to run in order after the current task finishes". 1.0.80: Enter "steers the prompt into the running turn, ctrl+q queues it". In the 1.0.105 bundle, remote sessions refuse with "`… cannot run immediately while the agent is working. Use the enqueue shortcut …`". The plausible worst case is that the compact is dropped or steered into the turn as text while the receipt says `delivered`. I see no path that corrupts the turn.
- **Cheap fix in scope.** After the brake passes, wait about 250 ms and re-read the registry row. Hold `recipient-busy` unless it is still `Idle`. A blip is under 5 ms, so a settled re-read nearly closes the window. It costs one SQL read and no launches, and stays out of the staged-submit code under R5.

### S4. A compact followed quickly by a body runs after the body, not before; the docs describe only the other direction
`docs/how/copilot-native-extension.md` and `pij-rs-api.md`

- **Failure scenario.** `pij send w --command compact` then `pij send w "<next task>"` a few seconds later. The extension's inbox wakes on the push and claims the body within milliseconds. The drain ticks every 5 s (`delivery_interval_secs: 5`, `config.rs:201`). The seat goes `working`, so the compact is held with backoff capped at 300 s (`pointer/mod.rs:110`), and it runs after the next task finishes. Claude keeps both rows on one kind, so the drain takes the compact first. The current docs say only that "a compact queued behind pending bodies may run before the later ones".
- **Fix.** Document the reverse case, and give C3 a line: for Copilot, let the compact land (or wait one drain tick) before dispatching the next packet. A code fix would block bodies behind due, never-deferred controls; that trades against S1-style blocking, so it is the prime's call.

## Nits

- **N1. The draft test does not use a Copilot frame.** `copilot_compact.rs:634` uses `╰──── hello ─╯`, the old box shape. M4 turns it red for the right reason, but it doesn't prove that a Copilot frame is read. My probe shows the canary's 1.0.105 frame is read correctly (`❯ hello world` with the caret at the end → `Recognized("hello world")`). Swap the test to that frame.
- **N2. A draft with the caret not at the end reads as an empty composer** (probe: `draft-caret-home -> Recognized("")`, `draft-caret-mid -> Recognized("hello")`). The gate then stages `/compact` into the human's draft, and `stage_submit` does not re-check the result (`crates/tmux/src/lib.rs:843-889`). This existed before the PR and Claude shares it. It is outside this PR under R5, but it now reaches Copilot panes for the first time. Worth a follow-up issue.
- **N3. Changing a seat's harness strands rows.** A `delivery-control:` row is stranded if a seat id changes from Copilot to another harness (the drain lists `delivery_kind` for it). The reverse case (a Claude compact left on `delivery:` reaching the Copilot extension, held without ack, `delivery/mod.rs:1817`) predates the PR. Both are rare.

## Verified (no finding)

| claim | how |
|---|---|
| Drain never claims a Copilot body | inventory maps Copilot → `control_kind` only (`worker.rs:123`); claim-time re-check `(harness==Copilot) != (kind==control)` (`worker.rs:233`); M3 receipt |
| Extension never claims a control | every native claim/park/recover uses `[delivery_kind(seat)]` (`delivery/mod.rs:559,1632,1907`); native ack refuses non-body kind or command (`:2162`) |
| Overlap guard on real SqliteQueue | `queue_copilot_control` 8/8 green; my lease-expiry probe confirms the serial key holds both ways |
| A control never runs twice | an expired control is failed "outcome unknown", never returned to pending (`queue.rs:179-215`; the pending reset requires `command IS NULL`) |
| Native-parking fix at queue.rs:754 | correct, and `\|\|` binds tighter than `!=` in SQLite; M6 receipt |
| Commands never carry FYIs | `carry = msg.command.is_none() …` (`delivery/mod.rs:1082`) |
| `new`/`reload` refuse; paneless Copilot compact refuses by name | `control_commands` 8/8 green, core unit, `native_tests` body endpoint |
| Every `delivery:` consumer | all SQL predicates in `queue.rs` checked. Changed: deferrals, admitted, require_delivery_job, outcome text. Body-only on purpose: renew/park/recover/claimed_delivery/heartbeat/terminal_state, all also `command IS NULL`. TS shim (`generation-routing.ts`) updated; `.copilot/` has no kind consumers (`rg --hidden`) |
| Producer kind | my extra mutant (`queue_kind` → always `delivery:`) turned red `control_copilot_compact_queues_on_the_control_kind…` and all 3 `copilot_compact` tests |
| Cost | no new launch site in the diff; a held pass reads the registry row already loaded; the canary spawn delta shows 0 for holds and 13 per delivered compact at existing sites. Loop: the existing 5 s drain tick plus one extra kind in an indexed `peek`, no per-seat launches |
| CI | `gh pr checks 95`: check(22), check(24), rust-lint, rust-test all pass |

## Commands run

- `cargo test -p pij-store --test queue_copilot_control` → 8 passed
- `cargo test -p pij-daemon --test copilot_compact --test control_commands` → 3 + 8 passed
- Extra mutant on `queue_kind` (restored, `touch`ed) → 4 red, as listed above
- Probes (removed from the tree afterwards): `zz_review95_probe.rs` (store), `zz_review95_wedge.rs` (daemon), `zz_review95_copilot_frame.rs` (harnesses)

## Not run, and why

- **A live mid-turn `/compact` in Copilot.** It needs a live Copilot session, which writes session state under `~/.copilot` (hands-off) and spends premium requests. My S3 evidence is the changelog and a static read of the bundle only.
- **TS typecheck/vitest locally.** No `node_modules` here, and the packet keeps me out of `node_modules`. CI ran them and passed.
- **Mutants M1, M2, M4, M5.** I relied on the coder's receipts. M3–M6 receipts were read for exit codes and red tests. I re-checked by reasoning that M4 and M5 fail for the right reason.
- **`harness checks`, live-fleet spawn measurement, `pij-rs ping`.** Read-only review, so I measured no live cost. The canary receipts stand.
