# Retro: pij-sympathetic-marten, F258 (daemon cost: the native inbox poll)

**Role:** coder/f258 for pm-001 (pij-steady-otter). The first OMP seat spawned in its own worktree (#304): cwd, relative paths and `AGENTS.md` all resolved to `pij-f258`; nothing landed in main.

**Diagnosis:** the daemon used 16–20% of a core with the fleet quiet. `GET /v1/inbox`'s 75–100 ms active per completion was one 55 s native long-poll re-running a claim pass every 500 ms (76 Copilot seats × 2 passes/s = 152 passes/s, about 900 SQLite statements/s). `POST /v1/whoami` at 8.4/s is the Copilot status line (76 sessions / 10 s refresh) plus a full `seats` scan per call; `/v1/cards` is chainglass's terminal server. Per-thread CPU came from `proc_pidinfo` thread times (no profiler attached): 57% tokio workers, 40% SQLite workers, 3% the tmux control reader.

**Cut (A1):** the native idle wait is event-driven: seat events, `Queue::next_claimable_in` (a due `not_before` or a lapsing claim) and a 15 s backstop. Four lost-wake tests on the real store; three mutants (no timer, no event wake, the 500 ms tick back) each caught.

**magicWand:** a per-thread CPU line in `ping` (tokio workers, SQLite workers, tmux control), read the same way this diagnosis read it. Route `active_us` is wall time and hides which pool burns the CPU; thread time names it without a profiler.

**difficulties:**
- One `harness checks` run failed `rust` in `pij-tmux --test control_mode`, a crate this change does not touch, under full-suite load (D-171-3's family); alone it passed 23/23, and the next full run was green.
- `harness checks`' error summary names a failed `rust` stage with no tail (`{"name":"rust","status":"fail","code":101}`); the output sits in a second `results` entry's `output` field, which a `jq` over the summary misses. A one-line clippy lint (`err_expect`) took a separate `just rust-check` run to see. D-pa-5's family.
- A paused-clock delivery test that ran 80 virtual seconds failed on a turn-wait hold for reasons outside this change (a receiver lease, likely); kept at 40 virtual seconds. `stretch:` name the fixture's lease horizon in the fixture.
- A 2.5 GB live store and 3,212 seat rows (F259) make pane lookups scan; B1 adds the index.
