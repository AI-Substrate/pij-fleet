# pij — Agent Rules

> **The harness is the product.** pij runs on two harnesses, layered:
> the **engineering harness** (`just` recipes, `harness/`, smoke/driver SDK,
> seed data — what humans and CI invoke) and the **agent harness** (the
> Boot/Interact/Observe loop, the skills in `skills/`, the retros
> ledger at `docs/retros/`, this file). The agent harness sits **on top
> of** the engineering one and cannot exist without it. Every extension is
> an exercise; every difficulty is a gift to encode; every agent run is a
> usability study. **If a session ends without one of the harnesses
> improving, something went wrong.** Ground on this at session start with
> `/eng-harness-flow` (or `harness boot`) before touching code.

## Governance

Follow the skill's canonical [Governance branch — the rules](skills/pij/references/prime/rituals/bootstrap.md#governance-branch--the-rules), including PRD reconciliation and store/document authority.
Resolve `<government-root>` with `harness/scripts/government-root.sh [REPO]` (REPO defaults to cwd); the linked bootstrap covers absent/unavailable worktrees.
pij's live requirements are on main in `docs/project/prds/`: PRD 001 (the community) and PRD 002 (`docs/project/prds/002-pij-base.dd.json`, the base register). Edit them with `ddocs`, then `ddocs validate` and `ddocs build`; `docs/project/README.md` has the rules.

## Inherited from pi-mono (do not violate without explicit user approval)

- No `any` types unless absolutely necessary.
- No inline imports — never `await import("./foo.js")`, never
  `import("pkg").Type` in type positions, no dynamic imports for types.
  Always top-level standard imports.
- Never hardcode keybindings; use a configurable matching object
  (`DEFAULT_*_KEYBINDINGS`).
- Biome check (errors and warnings) before commit: `just lint`.
- Type-check: `just typecheck` (`tsc --noEmit`).
- Tests: `just test`. Run from the package root.
- Read files in full before wide-ranging changes; do not rely solely on
  search snippets.
- Never use `git add -A` / `git add .`. Use specific file paths.
- Never bypass hooks (`--no-verify`, `--no-gpg-sign`).
- Never `git reset --hard`, `git checkout .`, `git clean -fd`,
  `git stash` without explicit user approval.

## pij-specific (Patterns P1–P10)

1. **T2 layout by default**: `.omp/extensions/<name>/{index,store,test}.ts`.
   T1 (single file) only for <80 LOC, single-concern extensions.
2. **Pi-free store**: `store.ts` imports nothing from `@earendil-works/*`.
3. **Inject side effects via constructor.** No global mutable state.
4. **Tagged-union returns** (`{ ok, ... }`) over throws.
5. **Constants live in `store.ts`** next to the data they constrain.
6. **Structural entry types** at the boundary (no cast at the call site).
7. **`.js` extension on relative imports** (NodeNext / ESM).
8. **Tests target the store**, not the wiring.
9. **Persist before mutate** (event-sourced consistency).
10. **One handler for `session_start`**, all reasons (`startup`, `reload`,
    `new`, `resume`, `fork`).

## Pij peer harness default

- Default new pij peers to **OMP** unless Jordan explicitly specifies another
  harness. Model/provider/effort instructions still override the default; always
  discover the exact selector at runtime and canary it before use.

## Is that check a POLICY or a BRAKE? (ask what REMOVING it does)

A check that reads a timestamp, a threshold, or any other input is one of two
things, and they have opposite consequences for what the change depends on:

- a **policy** — it decides the outcome, so it **inherits whatever its input is
  wrong about**;
- a **one-directional safety interlock** — a brake, which can only ever make the
  operation *more* conservative.

**You tell them apart by asking what REMOVING the check does.** If removing it
makes the operation do *more* (delete more, send more, kill more), it was a
**brake**. If removing it makes the operation do something *different*, it was a
**policy**.

**A one-directional safety interlock is not a policy.**

Worked example (a change in the earlier private repo, session s101). The orphaned-tap sweep reads each file's mtime
for a 5-minute grace, so "does it consult a timestamp?" is honestly **yes** — and
answering "no" would have been false while answering "yes, therefore it is
retention" would have blocked it behind an unrelated, still-open ruling on retention. The
resolving question was directional:

| input | decides deletion | can only spare |
|---|---|---|
| pane absent from `tmux list-panes` | ✅ the whole decision | |
| file mtime younger than the grace | | ✅ veto only |

**Age was never a reason to delete, only a reason not to** — remove the mtime
check and the sweep deletes *the same set or more*. So it is a brake, the change
depended on no ageing anchor, and it shipped independently.

Say which one it is **explicitly**, in those terms, when the change is
destructive: a reviewer looking at a 205MB deletion will otherwise assume a
retention rule governs it and go hunting for the policy.

## Resource cost is a correctness property (pij-fleet#62, plan 180)

For weeks the daemon launched **250–480 processes a second** (three tmux clients per pane every 600 ms, a `ps` per liveness check, a ~20-process status line, and bare-name launches that cost 18 kernel processes each on macOS). Defender's EDR inspects every launch and burned about two cores. Every test was green the whole time. **A green suite says nothing about cost**, so cost has to be checked on purpose.

Any plan, PR or feature that adds or changes a **loop, poll, timer, hook, status line, sweep or process launch** must, before merge:

1. **State its steady-state cost in the plan's backpressure**, as its own row: launches per second, poll interval, and how it scales with seats/panes (per tick × per seat is the shape that hurt us).
2. **Measure it on the live fleet, before and after**, and keep the receipts: `pij-rs --json ping | jq .data.spawns` (two readings a known number of seconds apart; per call site) and `just spawn-rate --seconds 20` (whole machine). See [`docs/how/macos-defender.md`](docs/how/macos-defender.md) § Measure launches.
3. **Launch nothing per tick per seat.** Prefer in-process work (read the file, call the syscall), the daemon's persistent tmux control client for tmux reads, or a cache with an explicit lifetime. Launch by **absolute path** (`/bin/ps`), never a bare name, in any repeated path.
4. **Record every new daemon launch site** with the spawn counter; `crates/testkit/tests/spawn_sites.rs` fails if you don't.
5. **Build and test nice-only, one build at a time; time real children honestly (F28, F186).** Every pij cargo build and test runs at niceness 19 only, one build at a time (shared-build's lock), with 4 jobs (`.cargo/config.toml`). `just` recipes nice themselves through `harness/scripts/nice19.sh`, which runs at max(19, the caller's niceness) and never stacks (F220): run them with no outer `nice`, and prefix `nice -n 19` only on a direct `node`, `vitest` or `cargo` call. Never `taskpolicy -b`: background QoS confines cargo to the efficiency cores and throttles its IO, so at fleet load builds starved for 20 to 35 minutes (Jordan's ruling of 2026-11-11, which supersedes plan 208's background-QoS rule and plan 209's `--no-run` split). `just shared-build` does all of this for you. Write a budget on a real `ps`, tmux or spawn as `pij_testkit::time::scaled(...)` (TS: `scaled()` in `harness/driver/time-scale.ts`); `PIJ_TEST_TIME_SCALE` (default 1) stretches it on a loaded machine, and production constants never read it.
6. **Never kill or delete by pattern in a shared root (D-C34-3, F262).** Every seat runs as the same user, so `pkill -f`, `killall` or a `ps | grep | kill` reaches every seat's matching process, not just yours: kill only the pid or process group of a job you started. The same holds for files in a shared root (a shared temp dir, `/tmp`, `$TMPDIR`, tmux's socket dir): a glob there reaches other seats' in-flight files. On 2026-11-21 a review seat's `rm -rf /var/folders/*/*/T/tmp.*` removed every `mktemp` dir for this user, other seats' builds and tests included. Delete only paths you created and recorded.

Reviewers check cost explicitly, the same as correctness: name the loop, its rate, and its per-seat scaling, or say there is none. Charge a process to a seat by pane-tree descent from the seat's recorded process, never by its inherited `PIJ_*` env, and never charge OMP's `__omp_worker_*` processes (and their subtrees) to a seat at all, even one they run under: they are machine-wide and carry the id of whichever seat started them (F203; [`docs/how/daemon-cost.md`](docs/how/daemon-cost.md) § Which seat a process belongs to).

## Searching this repo (known trap — silent, and it reads as absence)

**`rg` skips hidden paths by default, and the extension source lives under
`.omp/` and `.copilot/`.** So a repo-wide ripgrep sweep is structurally blind to the code you are
almost certainly looking for, and reports it as *not present*:

```
$ rg --files --glob '**/rust-runtime.ts'            # nothing
$ rg --files --hidden --glob '**/rust-runtime.ts'   # .omp/extensions/pij/adapters/rust-runtime.ts
```

**Always pass `--hidden` when sweeping this repo.** `grep -r` is unaffected.

Why this one bites harder than an ordinary tool default: **reading a file under
`.omp/` by explicit path works fine**, because that bypasses the traversal skip. So
a session can successfully open `.omp/extensions/...` minutes before a sweep returns
nothing for the same tree, and the absence feels *corroborated* rather than
suspicious. A tool that answers correctly when pointed and blindly when swept is
the worst available shape for this error.

Stated generally, because it is not really about ripgrep: **a probe's default scope
gets reported as a property of the repo.** "No matches anywhere" means *no matches
inside whatever this tool decided to look at*. Establish the scope before believing
an absence — an empty result is the one output that carries no evidence of what it
searched. (Found 2026-08-07 by `pij-massive-meadowlark` after three independent
citation disputes, all of which resolved as neither party being wrong.)

## Procedures: find it, or write it as you go (PRD 005)

Repeatable work has a procedure. `just proc` lists them (`pij proc` in any repo), `just proc <id>` shows a run and
`just proc <id> <step>` runs one step, refusing with its fix when a precondition fails. Find the procedure and follow it;
with none, write one as you go (`just proc PROC-meta-0001`), and fix a wrong step in the same PR. Each lives in one
folder with everything only it uses: `docs/project/procedures/<area>/`, or `skills/pij/procedures/` for those shipped
with the skill. `just procedures-check` is a `harness checks` stage. A rule below that is a step of a procedure keeps one
line here and links to it.

## Workflow

> **Canonical interface: `just`.** All composite gates live in the
> `justfile`; never compose npm or cargo steps by hand. `just` with no recipe
> lists every recipe.

0. **Fresh clone / new machine: `just install`** — single-command bootstrap:
   locked npm deps, build + link `pij-rs`, install OMP if absent, link the OMP
   and Copilot extensions plus curated OMP config, link the pij skill and the
   `pij` CLI shim, then `just doctor`. Re-run any time global state drifts.
   Start or restart the daemon with `just bounce-rs`; after an rs merge, follow
   PROC-daemon-0001 (`just proc PROC-daemon-0001 --key <PR>`).
1. Iterate on the OMP extension: run `omp` from the repo root and `/reload`.
   Type-check in another tab (`just typecheck`).
2. Test: `just test` (vitest) and `just rust-check` (fmt, clippy, cargo test).
   TS tests target `store.ts`-style pure modules.
3. Smoke: `just smoke` before merging.
4. **Before declaring any task done — or before ship — run `harness checks`.**
   The engineering-harness gate: it runs the full deterministic **signal
   inventory** (local-path portability → OMP worktree config → project docs (ddocs) → workstream coverage → procedures → pij skill structure → typecheck → lockfile allowlist → lint →
   test → Copilot native contracts → route inventory → Rust gate → bundled docs → smoke → commit-trailer
   attribution) as individual stages and reports a per-sensor verdict — and
   unlike `just self-check` it runs **all** sensors, so one invocation surfaces
   every failure (`--quick` skips the heavy route-inventory, Rust and smoke stages). **New
   back-pressure sensors get added to `.harness/extensions/checks/` and to
   `just self-check` together.** If it exits non-zero, the task is not done.
   The `/pre-commit` skill (`skills/pre-commit/SKILL.md`) encodes the full
   contract.

## Harness tooling

- **Driver SDK** at `harness/driver/` — typed `Scenario`/`Step`/`Session`
  for tmux-driven end-to-end smoke. `harness/scripts/smoke.ts` is a thin
  adapter over it; it runs every `.omp/extensions/<name>/smoke.ts`.
- **`just link`** — machine-wide policy from the **canonical main checkout only**.
  It refuses linked worktrees, links only `pij` into `~/.omp/agent/extensions/`
  plus `~/.omp/agent/{mcp.json,models.yml}` → this repo's `.omp/`, and links the
  native Copilot extension. `just unlink` removes only pij-owned links and
  pij-managed Claude hooks; real paths, foreign symlinks and foreign hooks are
  never clobbered.
- **`just rs-install`** builds `pij-rs` (release), installs a copy at
  `~/.local/share/pij/bin/pij-rs` and links it next to the `pij` shim in
  `$(npm prefix -g)/bin`; nothing on PATH points into `target/` (F187).
  **`just bounce-rs`** rebuilds, installs and restarts the daemon;
  **`just rs-autostart`** registers a macOS LaunchAgent. Both deploy recipes
  and `pij-rs daemon bounce` build through **`harness/scripts/release-build.sh`**
  (F226): it refuses build inputs that differ from HEAD (tracked or untracked
  under `crates/`, `Cargo.*`, `.cargo/` and the stamped extension trees; other
  local changes don't block), builds through shared-build into the checkout's
  one target (its lock, this tree's own sources, so another worktree's release
  artifacts can't be linked), stamped with HEAD, copies the binary out while
  still holding the lock (`shared-build --copy-out`), and hands on that private
  copy only if its `pij-rs --version` reads `pij-rs <version> (<HEAD, 10 chars>)`.
  The copy is what gets installed, so another tree's later build can't swap it.
  A failed build, dirty inputs or a wrong stamp stop the deploy before anything
  is installed or the daemon is stopped (failures after the daemon is signalled
  have no rollback, as before).
- **`just shared-build`**: any seat building into a cargo target another
  worktree also uses runs cargo through it, never bare:
  `PIJ_SHARED_TARGET=<abs dir> just shared-build -- <cargo args> [--then <cargo args>]`.
  Cargo names workspace artifacts by crate, so two trees building into one
  target overwrite each other mid-build. The recipe serialises builds with a
  lock, refuses under the disk floor (`PIJ_DISK_FLOOR_GB`, default 25), rebuilds
  your own sources with `CARGO_INCREMENTAL=0` (touching your crate roots only
  when another tree, or anything outside a finished run, built there since your
  last run: F243, `$target/.pij-shared-build-tree`), runs every cargo command at
  niceness max(19, the caller's) through `harness/scripts/nice19.sh` (never
  `taskpolicy -b`, F186; never stacked to 20, F220), and always releases. A stale lock is reported with its clear command, never
  cleared for you (plans 198, 208). A `cargo test` command may run at most
  `PIJ_SHARED_BUILD_TEST_LIMIT` minutes (default 20, stretched by
  `PIJ_TEST_TIME_SCALE`; `0` for none, `Ns` for seconds) from its start. Unless
  that is set, a run with a test name filter (any positional filter) also gets
  `PIJ_SHARED_BUILD_FILTERED_TEST_LIMIT` (default 3) from its first test
  binary. And every test run, whatever its limits, stops once it writes
  nothing for `PIJ_SHARED_BUILD_TEST_IDLE` (default 2; `0` for off) after its
  first test binary (F281). Past any of them the recipe names
  the limit, the last full `test ` line seen and what still runs in cargo's own
  process group, stops that group and only it, releases the lock and exits 8
  (F273). Strict on purpose (Jordan's ruling): a healthy test silent for more
  than about 180 s (120 s in a serial run) is stopped; raise a limit when real
  runs hit it. Parallelism is capped too, because `nice`
  lowers priority but does not shorten the run queue (F126). The repository's
  `.cargo/config.toml` sets `build.jobs = 4` and `RUST_TEST_THREADS = 4` for
  every cargo run in the repo, whatever its environment: recipes, `pij daemon
  bounce`, `pij-gate` and CI. Override with
  `CARGO_BUILD_JOBS`, or your own `RUST_TEST_THREADS` or `-- --test-threads N`.
  shared-build maps `PIJ_CARGO_JOBS` (default 4) onto both and prints the cap
  first: `cargo jobs: 4 (PIJ_CARGO_JOBS), test threads: 4 (RUST_TEST_THREADS)`.
- **One cargo target per checkout, and tests never build (F217).** A debug
  workspace build is about 15 GB, so a fresh `target/` per lane filled the disk.
  `harness/scripts/cargo-target.sh` names the one target: `CARGO_TARGET_DIR`,
  else `PIJ_SHARED_TARGET`, else the main checkout's `target/`, which every
  linked worktree shares. `just rust-check`, `route-inventory-check`, `smoke`
  and the `copilot-native-*` Rust recipes build into it through shared-build.
  The governance fixture never builds: it copies `<CARGO_TARGET_DIR or
  target>/debug/pij-rs` and runs the copy (`just smoke` builds it; CI builds
  `target/` first). Every recipe that runs a test runner (vitest, `node --test`,
  the smoke) or a cargo build nices itself through `harness/scripts/nice19.sh`
  (F220); load reached 198 from one un-niced `node --test`. Two sensors hold
  this: vitest puts `harness/test-tripwire/cargo` first on every test's PATH,
  so a test that starts a cargo build fails, and
  `harness/scripts/recipe-cost.test.ts` fails on an un-niced test or build
  recipe, a bare `nice -n` in any recipe (it stacks past 19 under a niced
  caller), cargo outside shared-build, an install of a `pij-rs` that
  `release-build.sh` did not build and stamp-check (F226), or
  a target not from `cargo-target.sh`.
- **Worktrees are plain checkouts (F219).** The committed project layer
  `.omp/config.yml` sets `worktree.clone: false`, so OMP's rewrite of
  `git worktree add` (and `github pr_checkout`) no longer clones the checkout,
  `target/` and `node_modules` included; deps come through `just worktree-deps`,
  cargo through the shared target. `just omp-config-check` (a `harness checks`
  stage) fails if the file or OMP's own answer from the repo root says otherwise.
  It is project-scoped: `just link` never links it, and `~/.omp` is not touched.
  OMP reads it from the OMP session's start directory (and an explicit
  `omp worktree add`'s cwd). Every worktree carries the same file, so start each
  OMP seat in its own worktree, never at main (#42; `--cwd <worktree>` there
  answers `false` too).
- **`just update-omp`** (procedure: PROC-release-0001) reinstalls OMP from the governed npm registry, then
  re-applies the guarded links and runs **`just omp-doctor`**.
- **`just update-copilot`** (procedure: PROC-release-0001) is the one way to take a Copilot CLI update: it runs
  `copilot update`, then `just copilot-sdk-contract` on the new version, and
  otherwise says **do not reload** (D-F208-2). A full pass records a local pass
  for that exact version in `~/.pij-rs/copilot-sdk-pass/` (F250): nothing to
  commit, no PR per version. `harness/copilot-sdk-verified.json` stays the
  committed floor CI reads. Copilot also updates itself: a `copilot-sdk` warn in
  `just doctor` or `harness checks` means run just copilot-sdk-contract before
  reloading seats; its pass clears the warn on this machine.
- **`just pr-freeze <pr>`**: is a PR head final for verdicts? Run by hand before each verdict
  request and before READY. FINAL (exit 0) only when `git ls-remote` and `gh` agree on the head,
  the base is main, it is CLEAN, every check's latest run passed, it is not draft, its migrations
  sit above main's highest, and its `Delivers:` line parses; otherwise each failing condition with
  its next step. `--worktree` creates the detached review worktree at that head (plan 220, F92).
  `just pr-freeze --prune [--dry-run]` removes `review-<pr>-<sha7>` worktrees whose PR merged,
  closed or moved to a new head (the one deciding input); local changes, a live seat's folder or the
  cwd of any of this user's processes (macOS lsof) inside one only ever spare it, and an unreadable
  seat or cwd list removes nothing (F107). `--prune` is macOS only; elsewhere it refuses. It runs on node's TypeScript stripping, with no node_modules, so it works from any review
  worktree (F109).
- **`just doctor`** — read-only audit: CLI shim shape, `pij-rs` on PATH, skill
  link, OMP policy, pij-managed Claude hooks, the Copilot extension and daemon
  health; each one missing or stale prints its fix. The first diagnostic when
  anything drifts.
- **npm supply chain** — `.npmrc` enforces a seven-day `min-release-age`
  quarantine; `just lockfile-allowlist` fails on any lockfile source outside the
  allowed registry host; CI runs `npm audit --audit-level=high` as a hard gate.
  Set `PIJ_NPM_REGISTRY` to resolve through a mirror instead of npmjs.
- **Engineering harness (`harness` CLI + `.harness/`)** — pij has adopted the
  ai-substrate engineering harness (governance doc `.harness/engineering-harness.md`,
  extensions in `.harness/extensions/`). Two verbs matter day-to-day:
  **`harness boot`** (fast readiness proof = typecheck + test) and
  **`harness checks`** (the full ship/done gate). `harness doctor` audits what
  loaded. The CLI is an ambient tool (global npm, never a repo dep); the
  `.harness/` substrate is committed.

## Voice input — phonetic interpretation

The user drives a lot of input via voice dictation. Expect occasional
homophone swaps, adjacent-word substitutions, and minor transcription
errors. When a word seems out of place, **try the phonetic neighbour
first** before asking — common patterns:

- "MPM" → `npm`
- "to do" → `todo` (the extension)
- "pee eye" / "pie" → `pi`
- "minnie h" / "mini h" → `minih`
- "yam'l" / "yarmel" → YAML
- "just file" → `justfile`
- "pre-checking" / "pre-check" → `pre-commit` (skill)
- "scale" → "skill"

If two phonetic candidates are both plausible **and** the choice changes
what code you'd write, ask according to the clarification protocol below.
Otherwise pick the one that fits the surrounding context and proceed — the
user prefers forward motion over interrogation.

## Clarification protocol

Before guessing, **ask**. When you'd otherwise type a question to the user
in plain prose, use your harness's structured question tool instead (for
example `ask_user_question` or `AskUserQuestion`), batching 1–4 questions in
one call.

**Pij orchestration exception**: a seat or peer in pij orchestration never
invokes `ask_user_question` or any modal question UI — it asks in ordinary
inline text, persists the pending decision, and blocks only dependent work;
question ownership stays with the context owner (parents relay pointers, never
proxy). This changes the transport and ownership path, not the requirement to
ask before consequential ambiguity. Full doctrine: `skills/pij/SKILL.md`
invariants 9–10 and `skills/pij/references/prime/protocol.md`.

**Pij worktree synchronization rule**: isolation removes edit-time
serialization, not convergence-time serialization — work confined to a verified
stream worktree/branch is notify-only; synchronize at convergence or any shared
mutable resource. Full doctrine: `skills/pij/SKILL.md` invariant 11 and
`skills/pij/references/prime/rituals/batons.md`.

Use it when:

- requirements are ambiguous, conflicting, or implied rather than stated
- you'd otherwise pick a non-obvious default that could surprise the user
- architectural trade-offs need a user opinion (e.g. T1 vs T2 layout,
  vitest vs node:test, sync vs eager install)
- you're about to take a destructive or hard-to-reverse action

How to use it well:

- Batch related questions in one call — don't ping-pong.
- 2–4 concrete options per question; if you'd recommend one, put it first
  with `(Recommended)` appended.
- The tool auto-adds an "Other" free-text fallback; do not add your own.
- `header` ≤ 12 chars; it's the tab label.
- Skip the tool only for trivially-answerable questions (single yes/no in
  the middle of a confirmed plan) — prefer it whenever ambiguity is real.

## Self-improvement loop

This is the core mechanism that makes the harness compound. **Not
optional** — every session contributes back.

**Magic wands** — every agent session should leave a `magicWand` (the one
thing the agent wishes were different) and its `difficulties` (structured
friction reports) in `docs/retros/<slug>.md`. **Read existing retros before
starting work on a surface** — they are feature requests from the most
honest users of this harness.

**Difficulty ledger** — every difficulty encountered → `docs/difficulties.md`
with severity. Every workaround → either an immediate fix (encode it) or a
wishlist entry (`stretch:` tag). Resolved items get curated into the
relevant skill or this file so future runs never hit them.

**Retros ledger** — `docs/retros/` is the canonical record of agent
sessions; append each session's retrospective under `docs/retros/<slug>.md`.

**Velocity log** — every phase end → row in `docs/velocity.md` with
start/end and output. Goal: each successive extension is faster than the
last (no fixed minute thresholds are gates).

**Encode, don't document** — a wiki paragraph that says "remember to do
X" is worth nothing; an automated step that does X for you is worth
everything. Prefer a recipe, generator, template, lint rule, or pre-flight
check over prose. Pick the right home: dev-loop friction → engineering
harness (`harness/`, `just` recipes); agent-side friction (skill confusion,
missing context, prompt regression) → agent harness (`skills/`, this file).

**Agents are real users.** Their `magicWand` feedback is feature requests
from your most honest user. Treat it that way.

## When something is unclear

- Run `/eng-harness-flow` (or `harness boot`) to re-ground on the
  philosophy + the self-improvement contract.
- Check `docs/retros/` for prior agent runs against the same surface
  — magic wands and difficulties from earlier sessions often pre-answer
  the question.
- The daemon's API and refusal contracts: `docs/how/pij-rs-api.md`.
- The OMP extension API is pi's (`@earendil-works/pi-coding-agent`); OMP is a
  pi fork, so its extension types come from that package.

## Forbidden without explicit user approval

- Modifying the installed OMP binary outside `just update-omp`.
- Skipping any of P1–P10 in a new extension.
- Replacing the toolchain (npm scripts → just/make/pnpm/etc.).
- Publishing to npm.
- Pushing to a public remote. **Standing exception (Jordan, 2026-10-09):** pushing
  feature branches and opening PRs on `AI-Substrate/pij-fleet` needs no per-case
  approval ("yes no need to ask for things like that"). Merging to `main` stays with
  the prime, after review and green CI. Never push `refs/notes/*` there.
