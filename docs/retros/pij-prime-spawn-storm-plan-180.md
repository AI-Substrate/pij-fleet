# pij-prime-spawn-storm: stop the pij spawn storm (plan 180)

**magicWand:** a seat should see its own pij queue. Twice this plan, prime
messages sat pending behind a long turn: six of them (two window-open calls,
two window-done reports, a broadcast and the ruling on the residual) for about
an hour. The prime ran windows 1 and 2 itself, and its ruling on the residual
`ps` load (job 4658) reached this seat only because it read `jobs` in
`~/.pij-rs/pij.sqlite` by hand. Pushed messages arrive only between turns, and
nothing inside a turn says any are waiting. `pij report now` (or `pij-rs
ping`, or the status line) should say "3 pending for you, oldest 41m", so a
seat ends its turn before it starves. Observed as DL-004.

**What shipped (four windows, all deployed by the prime):**

| window | build | what | deployed | drain |
|---|---|---|---|---|
| 1 | f1d9b81 | tmux reads over one control-mode client (`tmux -C`, read-only, no-output) | 14:03:27Z, pid 57934 | 44 s |
| 2 | 4b918d6 | Copilot status line as one process (`pij-rs statusline copilot`), bytes identical | 14:19:21Z, pid 42036 | 48 s |
| 3 | 893ae50 | follow-up C: `pij-rs ping` counts every daemon launch by call site | 14:44Z, pid 85679 | 57 s |
| 4 | 0acbf95 | follow-up D: `/bin/ps` and `/bin/date`, not a PATH search | 15:09:22Z, pid 33659 | 52 s |

The drains (44/48/57/52 s) all came close to the 60 s cap, with no SIGKILL needed.
Window 3's 57 s came closest. If the cap is ever tightened, or a drain grows with
seat count, this is where a bounce will first need a SIGKILL.

Rollback chain, each binary staged in `~/.pij-rs/bin/`:
0acbf95 → 893ae50 → 4b918d6 → f1d9b81, and the deep rollback d05776df (#71's
build, which reinstalls the plan-156 bash status line at boot).

| measure | baseline (23:43 local) | after window 4 |
|---|---|---|
| new processes/s, whole machine | 740.6 | 16.5 (−97.8%) |
| pij-rs daemon children/s (kinfo) | 250–480 | 6.6 |
| status line launches/s | ≥40 | 0 |
| Defender (epsext + wdavdaemon_enterprise + wdavdaemon) | 156.6% | 8.6% |

The prime's independent `measure-spawn` sample agrees: 17.1/s machine at 15:10:34Z,
daemon 5.1/s, Defender ~12–15%. Optional follow-ups (in-process liveness, and
resolving git/tmux once) are pij-fleet#74.

**Estimate vs actual** (local time, AEDT; the prime logged each for Vaughan):

| milestone | estimate | actual | |
|---|---|---|---|
| design sent | ~00:20 | 23:58 | 22 min early |
| phase 1 deploy-ready | ~02:45 | 00:31 | 2 h 14 min early |
| phase 2 deploy-ready | ~05:00 | 01:10 | 3 h 50 min early |
| C pushed / CI green | 02:25 / 03:00 | 01:39 / 01:43 | 46 / 77 min early |
| D CI green / window 4 | ~02:45 / ~02:55 | 02:07 / 02:09 | 38 / 46 min early |

Every milestone landed early. The phase estimates were sized for writing a
control-mode client and a status-line port from scratch. The tests-first loop
(RED, GREEN, mutants) ran faster than estimated because the goldens and the
launch-logging tmux wrapper caught mistakes within minutes. Pad the next
estimate less, but keep padding for measurement surprises, which is where the
real time went.

**The lesson that changed the outcome: two instruments that disagree are both
telling the truth.** After window 2, `proc_pidinfo` attribution
(`spawn-rate.py`) said the daemon launched 8.7/s and kinfo sampling said
39–79/s. The ruling was to build a per-site counter (C) rather than guess. Over
the same 60 s, the counter and kinfo agreed exactly on every `ps` and `git` that
ran (379 and 3). kinfo also saw 3,452 more children that never exec'd: failed
attempts. `Command::new("ps")` by bare name goes through `posix_spawnp`, which
tries every PATH entry. On macOS each miss creates and tears down a kernel
process, which `proc_pidinfo` cannot read and kinfo can. With the daemon's
32-entry PATH that was 18.11 PIDs per `ps` against 1.00 for `/bin/ps`, about
110 wasted process creations a second: 46–83% of every new process on the
machine. A one-line-per-site fix (D) did more for the machine total than
plan 181's proposed new dependency would have. Neither instrument alone found
it. The exact counter made kinfo's surplus impossible to dismiss as sampler
noise. Recorded as INS-001/INS-002.

**difficulties:**
- **Queue starvation** (above; DL-004). Severity high: it moved two deploys to
  the prime and delayed a ruling an hour. Workaround: end turns at milestones,
  and `sqlite3 -readonly ~/.pij-rs/pij.sqlite "SELECT id, json_extract(payload,'$.body') FROM jobs WHERE serial_key='<seat>' AND state='pending'"`.
- **Sharing `CARGO_TARGET_DIR` between two worktrees of one workspace**
  silently built the other checkout's `pij-tmux`: path-crate artifacts are not
  keyed by path. The compile error looked like my source had lost a method.
  Severity medium (DL-001). Fix: separate target dirs per worktree.
- **An in-process daemon in `#[tokio::test]` (current_thread) deadlocks** when
  the test blocks on a child process that calls the daemon. Use
  `flavor = "multi_thread"`. Severity medium (DL-002).
- **rustfmt reflow broke scripted edits three times**, the same trap each
  time: an unasserted `str.replace` matched nothing after rustfmt rewrapped an
  `assert!`. It cost a surviving mutant (S6), a wrong GREEN run on the counter
  test, and a clippy loop on a date literal. Asserting the match count caught
  the later two. Severity medium (DL-003). `stretch:` edit after `cargo fmt`,
  or use the edit tool against the formatted text.
- **Truncated failure output hid CI-only failures.** Piping a local run through
  `head -20` hid two harness-script test failures that CI then caught on
  9d304a2. Severity medium. Fix: filter for `FAILED|panicked|test result`, never
  head.
- **A locale-dependent assertion** passed on macOS and failed on ubuntu CI
  (tmux renders the tab/✓ differently without a UTF-8 locale). Fixed in
  9d304a2 by asserting only the frame line. Severity low.
- **Known load flakes** (pij-tmux adapter 1–2 s deadlines, pij-harnesses
  claude_settings hooks) failed in parallel runs and passed alone. Proving
  "not mine" each time took 2–4 extra cargo runs. Severity low and
  pre-existing; same `stretch:` as plan 179 (a `--serial-retry` mode).
- Papercuts: the bash tool refuses any command text containing the word "kill"
  without a numeric PID; `pij report state working` is invalid (the states are
  ready, waiting, hold, blocked, question, done, failed, cancelled).

**gifts:**
- The injection guard's `\r` branch had no test, so its mutant SURVIVED: a
  guard over a character set needs one mutant per member, not one for the set.
  A carriage-return assertion was added and the mutant is now killed (GFT-001).
- The launch-site guard (`spawn_sites.rs`) caught its own false positive on its
  first run (`operation.command()` returns a name, not a `Command`), and MC6
  proves it fails when it stops seeing builder calls.
- The decoy-on-PATH test carries its own control (a bare name must find the
  decoy), so it cannot pass vacuously; mutant MD6 proves the control bites.
