# Independent cross-family review — plan 175, PR #59, 617fc02

**Verdict: REQUEST CHANGES.** Non-OK re-survey rows: **7 WRONG, 0 STALE, 3 MISSING, 0 BLOAT, 3 UNVERIFIABLE**. The two coverage gates pass, but the normal OMP-in-a-stream-worktree spawn path is rejected before the daemon call, the pair route can claim a gate ran before running it, and mandatory governance evidence paths do not exist in this repository. WRONG rows must be fixed before merge. All 29 Markdown files and the two JSON files under `skills/pij/` were read; this is one partition. `pij`/`pij-rs --help` and checkout code are the primary evidence; historical/operational claims without such evidence are called out below rather than treated as verified.

## Re-survey — per-file non-OK tables

`skills/pij/references/routes/pair.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 78-85 | Spawn the default OMP coder into `<worktree>` with `pij-rs spawn --cwd <worktree>` | `crates/cli/src/main.rs:2649-2657` rejects **OMP and Pi** whenever `--cwd` resolves to a linked git worktree, *before* `/v1/spawn`. `git rev-parse --git-dir` here resolves to `.git/worktrees/fleet-175-review`; ordinary stream worktrees have this shape. The diagnostic says spawn from main then cd afterward, incompatible with the route's `--cwd <worktree>` and worktree-bound descriptor. The default harness is OMP (`pair.md:84-85`). This blocks the documented normal path; code fix or explicitly workable alternate route needed. | WRONG |
| 33-35, 60-61, 114-127 | Report "`Approved ... after harness checks`" after reviewer verdict, **before** sending report pointer | Own procedure step 4 records approval immediately after verdict; step 5 only then runs `harness checks`. This ordering makes the mandatory report claim a result that has not happened yet; `orchestrator.md:19-21` repeats the after-checks claim. Move the check before approval/report or move report after gate success. | WRONG |

`skills/pij/references/routes/delegate.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 36 | `pij-rs spawn --harness <h> ... --cwd <absolute-worktree>` is the ordinary acquisition step | For the skill's OMP default this rejects linked worktrees at `crates/cli/src/main.rs:2649-2657`. See pair row; a plain native spawn from this linked checkout with OMP cannot even reach registration. | WRONG |

`skills/pij/references/prime/orchestrator.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 55-64 | Default OMP coder and native spawn `--cwd` verified stream worktree | This exact combination is refused by `crates/cli/src/main.rs:2649-2657`. `pij-rs spawn --help` exposes `--cwd`, but its preparatory validation rejects the documented default in a normal linked worktree. | WRONG |

`skills/pij/references/prime/rituals/kickoff.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 15-19 | Construct stream worktree, then native-spawn OMP/Pi `--cwd <absolute-worktree>` | `crates/cli/src/main.rs:2649-2657` refuses these harnesses for linked worktrees. The documented default OMP stream cannot complete step 7. | WRONG |
| 46-51 | Write mandatory pass-time canary record under `<government-root>/canaries/` | `harness/scripts/government-root.sh` exits 1 here: "expected one standing prime-governance worktree; found 0." `bootstrap.md:46` and ruling R2 explicitly forbid inventing that root or creating the branch without human go. There is no record path for pij-fleet, so the required three-leg canary audit cannot be filed as directed. | WRONG |

`skills/pij/references/prime/rituals/incidents.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 20-24 | At detection, open `<government-root>/incidents/INC-NNN.md` | Same resolver exit 1 and R2, `bootstrap.md:46`. No absent-government fallback is specified, so a pij-fleet incident cannot follow its mandatory pass-time recording procedure without violating the ruling. | WRONG |

`skills/pij/references/prime/templates/stream-brief.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 29-32 | Canonical prose fence link is `<government-root>/spine.md#...` | For pij-fleet this resolves nowhere (resolver exit 1, R2). Name `pij fence show --stream <id>` or an extant plan-file pointer as the store-native/absent-government case; otherwise generated briefs contain a dead authority pointer. `node.md:34` documents the shipped fence view. | MISSING |

`skills/pij/references/prime/orient-global.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 37-39 | Canary records go to disk at pass time | True as policy, but no legal destination exists for this repo's no-government case; `kickoff.md:50` mandates the absent government root and `bootstrap.md:46` disallows creating it. Name the store receipt and/or plan-owned record fallback. | MISSING |

`skills/pij/references/routes/node.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 53-55, 65 | `report now ... [--note]` presented as a generally usable option | `crates/daemon/src/http/report.rs:224-240`: `--note` is accepted **only** when the same `report now` also passes `--state question` or `--state blocked`; the bare note and notes on `done` refuse. Document this prerequisite next to the option. | MISSING |

`skills/pij/references/00-routing.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 216 | Plan 083 fleet measured ~2.9M A2A tokens | `find/rg --hidden` inside this checkout finds no `docs/plans/083-*`; the ledger's rejected survey row co-38 points to a different, pre-OSS checkout outside this packet's evidence. Not independently corroborated by current code/help. | UNVERIFIABLE |
| 248 | A prior uncapped run reached load ~230 on 2026-09-29 | Historical operator anecdote, no local receipt/code can prove the number; operational caps are instructions, not dependent on this figure. | UNVERIFIABLE |

`skills/pij/references/prime/protocol.md`:

| line | claim | reality + evidence | verdict |
|---|---|---|---|
| 115 | Historical RCA figures 38.6 h / 84% / 64% / 15% / 10% | Attribution names an external workspace, not checkout code/help or a vendored measurement. Treat numbers as explicitly attributed, not independently verified here. | UNVERIFIABLE |

The remaining Markdown files (`SKILL.md`, `routes/{agent,ops,peer,prime,ready,skill}.md`, `prime/{orient-oprime,pa}.md`, `prime/rituals/{batons,bootstrap,reports,store-native}.md`, `prime/templates/{baton-book,orient-local,seat-handover,spine}.md`, and `prime/exemplars/{canary-record,grant-log}.md`) produced no additional non-OK row in this pass. `prime/{biome.json,prime-flow.schema.json}` contain no additional CLI claim. The plain records and labeled historical exemplars were not treated as fresh live receipts. No repo edits were made.

## Ledger spot-check (minimum 15 FIXED)

| ID | Patched claim | Ground truth and result |
|---|---|---|
| co-08 | `SKILL.md:41-42` force/reason | `pij send --help` exposes `--force --reason`; **confirmed** |
| co-18 | `SKILL.md:88` any-seat watchdog opt-in | `pij-rs watchdog --help`, `crates/core/src/watchdog.rs:388-390`; **confirmed** |
| co-28 | `00-routing.md:45` working link | `pij --help` POST `/v1/link`, `crates/daemon/src/http/role.rs:615-646`; **confirmed** |
| co-30 | `00-routing.md:101,111` no `--layout` | `pij-rs spawn --help` has `--name`, no layout; **confirmed** |
| co-33 | `00-routing.md:204` self-send refusal | `crates/core/src/error.rs:140`; **confirmed** |
| co-34 | `00-routing.md:206-208` death sweep instead of TS boot/no-show | `crates/daemon/src/death_sweep.rs:18,71,81-94`; **confirmed** |
| ro-16 | `routes/ops.md:54-59` revive after tombstone / dead+absent | `pij-rs revive --help`, `crates/daemon/src/http/revive.rs:110-139,245-310`; **confirmed** |
| ro-25 | `routes/ready.md:20-23` OMP/Pi self-register, not adopt prohibition | CLI allows `pi`; `crates/daemon/src/http/identity_tests.rs:1309-1320` adopts both; guidance avoids duplicate extension registration; **confirmed** |
| ro-28 | `routes/node.md:12` real roles | `crates/core/src/orchestration/governance.rs:161-179`; **confirmed** |
| ro-29 | `routes/node.md:101` link served/tree refused | `pij --help`; **confirmed** |
| ro-30 | `routes/node.md:56-65` question/blocked flags and no note flag | `crates/daemon/src/http/report.rs:255-286`; **confirmed** (separate `report now --note` omission above) |
| pc-23 | `prime/orient-oprime.md:94-96` no anomaly push; PA digest | `crates/daemon/src/pa_watchdog.rs:921,1007`, anomalies read route; **confirmed** |
| pr-09 | `prime/rituals/kickoff.md:21-25` link works | `pij --help`, `crates/daemon/src/http/role.rs:615-646`; **confirmed** |
| pr-21 | `prime/rituals/store-native.md:21` project filter is not inherited | `crates/daemon/src/http/governance.rs:992-1015,1492-1506`; **confirmed** |
| pr-25 | `prime/templates/stream-brief.md:65` role pm | `crates/core/src/orchestration/governance.rs:161`; **confirmed** |

**REJECTED co-38:** the ledger says plan 083 exists in another checkout; no local source or CLI verifies the quoted measurement, so that row is **UNVERIFIABLE here**, not independently supported as REJECTED. **RULED sample:** R1 `pair.md:1-10` replaces flow-pair machinery with two builder seats; R2 `bootstrap.md:46` correctly states absent government (but kickoff/incidents fail to follow it); R3 `SKILL.md:55` keeps shim chore refused despite native sidecar; R4/R5 `protocol.md:122-123` align idle messaging and governed compaction; R6 `SKILL.md:77` names only current CLI-owned flow files; R7 `ops.md:57` retains manual duplicate-conversation check.

## Gates, scope, judgements

- `npx vitest run .omp/extensions/pij/core/generation-skill-coverage.test.ts` **exit 0, 7/7**. Source imports `RS_ROUTE_TABLE` directly from `generation-routing.js` and checks both directions including leaves; no copied route list.
- `CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-175-skill-realign/target nice -n 19 timeout -k 10 1800 cargo test -j 1 -p pij-cli --test skill_verb_coverage` **exit 0, 4/4**. Rust test uses `CARGO_BIN_EXE_pij-rs --help`, parses clap `Commands:`, checks omissions and mutation; no hard-coded native verb list. `sysctl -n vm.loadavg` was **11.25 12.85 12.51** before build (<40; no pause required). Both tests gate verb inventory, not procedural accuracy.
- `git diff --stat bb9f0b1 617fc02`: **54 files; 2,660 insertions / 481 deletions**. Nothing outside `skills/pij/`, the two tests, `docs/how/`, `docs/retros/`, and plan 175 folder. Checkout was clean and HEAD `617fc02`; `git diff --check` exit 0. `find skills/pij -name '*.md' | xargs wc -l` = **2,491**, under 2,800. Link checker run directly: **121 checked, 0 broken, exit 0**.
- **(a) OK with a runtime qualifier:** `orchestrator.md:55-59` defaults coder OMP `github-copilot/gpt-5.6-sol --effort xhigh` and asks for a reviewer of another family; `crates/harnesses/src/spawn.rs:169` maps OMP effort to `--thinking`, and `models.rs:209-236` knows the Copilot GPT-5.6 family. Reviewer still must be discovered in the installed harness catalog and canaried; no fixed reviewed-family selector is asserted. The **linked-worktree spawn blocker above is separate and decisive**.
- **(b) OK:** `docs/how/pij-rs-api.md:557` FYI link now points to `skills/pij/references/00-routing.md#c10--wire-discipline-a2a-messages`, rule 3. The target anchor exists; link checker agrees.
- **(c) OK for plan AC11, not a permanent gate:** `assets/scripts/linkcheck.py` is exercised manually and the receipt/mutation output is in `assets/receipts/tk-0005-size-links.txt`; AC11 asks for a script receipt, not new CI wiring. No `just`/test/CI invocation references it, so future link edits can drift; wiring it later would improve backpressure but is not this acceptance criterion.

**Repair priority:** first reconcile native OMP linked-worktree rejection with the default stream spawn recipe (or explicitly change the default/handoff with evidence); then order `harness checks` before claiming an approved phase; then supply R2-compatible pass-time canary/incident record destinations without fabricating a governance worktree. No mutations to the repo, daemon, tmux, or production environment were made.
