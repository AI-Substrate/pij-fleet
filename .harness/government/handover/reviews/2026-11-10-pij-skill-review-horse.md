# pij skill review — fresh reader (pij-elaborate-horse, 2026-11-10)

Read-only. Tree: main @ e10198ba. No files changed.

## Part 1 — FYI wording (feeds F169)

**Caveat on "cold":** the brief stated Jordan's intent before I read any FYI text, so my first reading was not blind. What follows is which words pull which way, read as an agent choosing a flag mid-task.

### 1. What the text says `--fyi` is for

Three different tests are in circulation, and they do not agree:

| # | Test the text gives | Where | Pulls toward |
|---|---|---|---|
| A | "Use only when the recipient's **next action doesn't depend on it**" | `pij-rs send --help`; `cli.ts:25`; both tool descriptions; C10 rule 3; `prime.md:59` | ambiguous (see below) |
| B | "A normal send lands inside a turn the recipient is already in, so **choose `--fyi` unless their current work depends on it**" | `SKILL.md:42`; `.copilot/…/extension.mjs:90`; `.omp/…/index.ts:95` | **FYI as the default** |
| C | "held until the seat's next turn … **opens no turn**"; "$0 now"; "every send costs the recipient's whole context … no acks" | `peer.md:73,83`; cold-wake refusal (`crates/core/src/cold_wake.rs:291`); `prime.md:59`; `protocol.md:118-120` | **FYI as the cheap, polite choice** |

So what I took `--fyi` to be for: *the considerate, zero-cost way to tell someone something when I am not waiting on their reply.* Test B says so outright ("choose fyi unless"), and it sits in the tool description, the one text an OMP/Copilot agent always has in context.

Test A is the intended rule but it is phrased from the wrong side. "Their next action doesn't depend on it" asks me to model the recipient's plan. For a hand-off the recipient is idle: it has no next action for my message to be a dependency *of*. My message does not unblock their action, it *is* the cause of it. Read literally, "your turn" to an idle seat passes test A.

Nothing anywhere states the consequence in plain words: **an FYI never wakes anyone; an idle recipient will not see it until something else wakes them.** "Held until the recipient's next real turn" reads as "slightly later", not "possibly never".

### 2. Would an agent handing over pick `--fyi`? Yes, likely. Five pulls:

1. **Test B makes FYI the default** and keys on the recipient's *current work*. "Step 3 done, your turn" is not about their current work, so B says fyi. It directly contradicts "use only when" two sentences later in the same description.
2. **The word.** "FYI: phase 2 is done" is ordinary English. The sender's mental test becomes "do I need a reply?" (no), not "must they act?" (yes). Role sends reinforce it: `--reply wake|fyi|none` uses `fyi` to mean "how I hear back".
3. **The never-list is the only thing protecting hand-offs, and it is a list, not a test.** "work done … hand-offs" is in `--help` and the tool param, but sits mid-paragraph after the dependency test; "ready", "your turn", "step done" are not literally on it. Lists get pattern-matched, and the nearest example on the FYI side is "merged #452, nothing needed from you", which looks just like a work-done note.
4. **The cold-wake refusal offers `--fyi` first, priced "$0 now", with no test.** A hand-off to a cold seat is refused and the top row of the fix-it table is `--fyi  hold for its next turn  $0 now`. `peer.md:85` says "Hold it with `--fyi`, or add `--force`". An agent following the error text converts an actionable message into a held one. I'd expect this to be the largest single source of stalls.
5. **Cost doctrine all points one way.** "No acks", "every send costs the whole context", "no blind messages … Message only a seat that must act" (`pa.md:102`). A seat finishing work wants to be cheap; FYI is the cheap verb.

The `peer.md` § FYI paragraph is all mechanics and defers "when to use it" to C10 rule 3 in another file. An agent that loaded only the peer route (progressive disclosure) never sees the rule, only `--fyi 'progress note'  # held; opens no turn`.

### 3. Suggested wording

One sentence, identical everywhere, placed **first**:

> **Does the recipient have to do anything because of this message? Yes: normal send, even if you need no reply. No: `--fyi`. An FYI never wakes anyone; an idle seat will not see it until something else wakes it.**

Exact replacements:

**a. `crates/cli/src/main.rs:390-398` (`--fyi` doc):**
```
/// Only for information the recipient does not have to act on. Test: does the
/// recipient have to do anything because of this message? Yes: normal send,
/// even if you need no reply. No: `--fyi`. If they'd be fine never reading
/// it, don't send it. Never `--fyi`: "done, your turn", hand-offs, "ready",
/// work or a phase complete, review verdicts, blockers, questions, decisions.
/// An FYI never wakes anyone: it is held until something else gives the
/// recipient a turn, so an idle seat may never see it. Five pending flush as
/// one message once the seat is warm. Receipt: `held (fyi)`.
```
and the example at `:354`: `--fyi --body 'merged #452, nothing needed from you' (nothing for them to do: held, wakes no one)`.

**b. `.omp/extensions/pij/cli.ts:25-31`:** same text as (a), replacing "Use `--fyi` only when … decisions needed."

**c. Tool descriptions, `.copilot/extensions/pij/extension.mjs:90` and `.omp/extensions/pij/index.ts:91,95`:** delete "A normal send lands inside a turn the recipient is already in, so choose fyi unless their current work depends on it." Replace the "Use `fyi: true` only when … decisions needed." run with:
```
`fyi: true` is only for information the recipient does not have to act on. Test: does the recipient have to do anything because of this message? Yes: normal send, even if you need no reply. No: fyi. Never fyi: "done, your turn", hand-offs, "ready", work or a phase complete, review verdicts, blockers, questions, decisions. An FYI never wakes anyone; an idle seat will not see it until something else wakes it.
```
**`fyi` parameter (`extension.mjs:103`, `index.ts:119`):**
```
true = hold until the recipient's next turn; it wakes no one. Only when the recipient does not have to do anything because of this message. Never for "done, your turn", hand-offs, "ready", verdicts, blockers, questions or decisions, even when you need no reply. Never with force.
```

**d. `skills/pij/SKILL.md:42`:** replace "`send --fyi` holds text the seat's next action doesn't depend on; a normal send lands inside a turn the recipient is already in (C31: …), so choose `--fyi` unless their current work depends on it;" with:
```
`send --fyi` is only for information the recipient does not have to act on, and it wakes no one; if the recipient has to do anything (done-your-turn, hand-off, ready, verdict, blocker, question, decision) it is a normal send even when you need no reply; a normal send lands inside a turn the recipient is already in (C31: …);
```

**e. `skills/pij/references/00-routing.md:220` (C10 rule 3), last sentence:**
```
`--fyi` is only for information the recipient does not have to act on. Test: does the recipient have to do anything because of this message? Yes: normal send, even if you need no reply ("done, your turn", hand-offs, "ready", verdicts, blockers, questions, decisions). No: `--fyi`. Fine never reading it: don't send. An FYI wakes no one.
```
`:38` row label: `FYI (recipient has nothing to do; held, wakes no one)`.

**f. `skills/pij/references/routes/peer.md`:**
- `:73` → `pij send <seat> --fyi 'merged #452, nothing for you to do'  # held; wakes no one; never for a hand-off`
- `:83` open the paragraph with the test instead of ending on a pointer: `**FYI: only for information the recipient does not have to act on.** If they have to do anything (done-your-turn, hand-off, ready, verdict, blocker, question, decision) send normally, even when you need no reply. …mechanics…` and drop "When to use it: § C10 rule 3".
- `:85` → "If the recipient has nothing to do with it, hold it with `--fyi`; if they have to act, `--force --reason '<why>'` (or let their parent decide)."

**g. `skills/pij/references/routes/prime.md:59` and `prime/protocol.md:120`:** "`--fyi` for what their next action doesn't depend on" → "`--fyi` only for what the recipient does not have to act on; a hand-off or verdict is a normal send".

**h. `.omp/extensions/pij/AGENTS.md:67` and `docs/how/pij-rs-api.md:743`:** "only when their next action doesn't depend on it" → "only when they do not have to do anything because of it".

**Encode, don't only document (two mechanisms, both small):**
- **Cold-wake refusal, `crates/core/src/cold_wake.rs:291`:** the `--fyi` row's effect text "hold for its next turn" → "only if they need do nothing; wakes no one". The row order (cheapest first) currently teaches the wrong habit.
- **Receipt, `crates/cli/src/main.rs:2970`:** `pij send: held (fyi) — <id>` → `pij send: held (fyi), <seat> was NOT woken — <id>`. The `?` warning (`crates/core/src/fyi.rs:39`) could also fire on hand-off words (`your turn`, `ready`, `done`, `handing`, `over to you`, `blocked`, `verdict`). Advisory only, as today.

## Part 2 — Does the skill match what pij does?

**Verdict: yes, closely.** Five Sonnet subagents checked about 300 concrete claims (verbs, flags, defaults, refusal codes, constants) across `SKILL.md`, `00-routing.md` and all ten route files against `pij-rs … --help`, `crates/`, the shim route table and the justfile. They reported one wrong command and a handful of omissions. I re-checked each item below against source or help myself; nothing was run that writes.

| Rank | Skill file:line | Says | Actually | Fix |
|---|---|---|---|---|
| 1 wrong command | `routes/community.md:20` | `pij directory add '<id>' --why … --title … --rule … --packet …  # no template` | `pij-rs directory add --help`: "Required, unless --file or --from template supplies them: --why, --title, **--kind, --lifecycle, --holders**, at least one --rule, and one of --packet or --brief". The schema failure is mapped back to those flags at `crates/daemon/src/community/ddocs.rs:130`. As typed, refused. (Confirmed from help and source, not by running it: it writes.) | Add `--kind specialist\|general --lifecycle long-lived\|per-task --holders one\|many` to the line. `:19` (`--from file:`) ends in `...`; say the same three are needed there. |
| 2 wrong guidance | `SKILL.md:42` | "choose `--fyi` unless their current work depends on it" | Contradicts the `--help` rule ("use only when") and Jordan's intent; see Part 1 | Part 1 fix (d) |
| 3 missing | `SKILL.md:56` | `directory` verb list: add … memory | `directory external add <seat> --from-project <slug> --why <note> [--role <id>]` exists (`crates/daemon/src/http/directory.rs:1749-1753`) and `community.md:38` uses it. The coverage tests only gate top-level verbs, so the gap is invisible to them | Add `external add` to the row |
| 4 missing | `SKILL.md:58`, `:56`, `:42`, `:52` | cites "C23", "C28", "C30", "C31" | The skill's own C-namespace is `00-routing.md` C1–C12. C28/C30 are headings in `community.md:42-46`, C31 is in `docs/how/pij-rs-api.md`; C23 is defined nowhere under `skills/pij`. A reader told to "cite § C…" looks in Shared conventions and finds nothing | Link each on first use, or rename the rulings so they don't share the "C" prefix with the conventions |
| 5 wording | `routes/peer.md:39` | `pij-rs spawn --harness <…> --model <exact-model> [--effort …]` (model unbracketed, reads as required) | `--model` is optional; only `--harness` is required | `[--model <exact-model>]` |
| 6 wording | `routes/peer.md:48` | cites `crates/cli/src/main.rs:2650` for the linked-worktree refusal | That line is `doctor` output. The refusal is at `main.rs:4072` | Cite the function (`is_linked_worktree`), not a line number |
| 7 wording | `SKILL.md:45` | `link <seat> --role <pm, worker or pa>` | Daemon usage is `link <seat> [--parent <you>] --role <pm\|worker\|pa>` (`crates/daemon/src/http/role.rs:560`) | Add `[--parent <you>]` |
| 8 wording | `routes/node.md:58-59` | bare `pij report clear` | `report clear [--assignment <id>]` | Add the optional flag |
| 9 wording (not the skill) | `.omp/extensions/pij/index.ts:91,96`, `.copilot/…/extension.mjs:90`, `cli.ts:32` | cold = "idle past its prompt-cache TTL" | The guard is a fixed hour: `idle_ms <= COLD_IDLE_MS` is Clear (`crates/core/src/cold_wake.rs:192`). `peer.md:85` has it right | "idle over an hour" |

**Subagents could not confirm either way** (no contradicting evidence found; not counted above): the PA timing table in `community.md` (20/30/10 min, 5-min compaction wake, 60-min idle nudge); `--fyi` refused for `seat@machine` and tombstoned seats (`peer.md:83`); paneless seats refusing `--command compact` (`pair.md:46`); `revive.legacy-tombstone` (`peer.md:62`); repeat-close and reap "reincarnated/unverifiable" wording (`ops.md:46-48`).

**Reported and dropped after checking:** `pij sessions` having no `pij-rs` twin (the skill only ever writes `pij sessions`, which is correct); `adopt --reclaim` missing from `peer.md` (it is in `ops.md:57`, where recovery lives); cold-wake "over an hour" in `peer.md:85` (correct; the tool text is the stale one, row 9).

**Clear and works well**
- `/pij ready` worked first time, cold: one adopt, two verifying reads, stop. The "hard boundary" list stopped me exploring.
- `00-routing.md` C7 (`pij bg`) matches help and daemon constants nearly line for line.
- The shim-refuses / native-serves split (`spawn`, `revive`, `tail`, `daemon`, `models`) is exact, so an agent never types a dead grammar.
- `node.md` Tasks and reports: states, 280/200 limits, `--note` rule and `status-stale` all match code.
- Refusal codes: every `E-RS-*` the skill names exists in code.

**One reader's note on shape:** the `SKILL.md` coverage table has cells of 1,500+ characters (`:42`, `:56`). The FYI rule sits in the middle of one. Rules an agent must not miss should not live inside a table cell.

## Top 5 fixes

1. **Remove "choose `--fyi` unless their current work depends on it"** from `SKILL.md:42` and both `pij_send` tool descriptions, and lead every FYI passage with the one test: *does the recipient have to do anything?*
2. **Cold-wake refusal** (`crates/core/src/cold_wake.rs:291`): stop offering `--fyi` first and unconditionally. It converts a refused hand-off into a held one.
3. **Say the consequence in the receipt:** `held (fyi), <seat> was NOT woken`.
4. **`community.md:20`:** add `--kind --lifecycle --holders`; the documented no-template command is refused.
5. **`SKILL.md:56` and the C-number cites:** add `directory external add`; link or rename C23/C28/C30/C31.
