---
record_kind: "retro"
harness_version: "0.14.0"
branch: "pij-prime/spawn-storm"
repo: "https://github.com/AI-Substrate/pij-fleet.git"
created_at: "2026-10-27T15:20:52.438Z"
agent: agent
plan_id: "180-spawn-storm"
schema_version: "1.2"
retro_id: "2026-10-27T15:21:00Z-agent-p180a"
started_at: "2026-10-27T14:11:15.219Z"
ended_at: "2026-10-27T15:22:00Z"
summary: "retro --drain session-end save (7 entries). Plan 180 (pij spawn storm) by seat pij-interesting-capybara for prime pij-broad-rykard: four windows deployed; machine 740.6 -> 16.5 new processes/s, Defender 156.6% -> 8.6%; ac-0006 met. Narrative: docs/retros/pij-prime-spawn-storm-plan-180.md."
entries:
  - id: DL-001
    kind: difficulty
    description: "sharing CARGO_TARGET_DIR between two worktrees of the same workspace silently built the base checkout's pij-tmux in place of the branch's (path-crate artifacts are not path-keyed); the compile error looked like my source had lost a method"
    target: tooling
    severity: degrading
    workaround: "separate target dirs per worktree; cargo clean -p on the collided crate"
    suggested_encoding: "just worktree-deps (or harness doctor) warns when CARGO_TARGET_DIR is shared across worktrees of one workspace"
    fp: "c00ad81049e8"
    disposition: kept
    system:
      compound:
        status: open
        source: agent-self
        first_seen_at: "2026-10-27T14:11:15.219Z"
  - id: DL-002
    kind: difficulty
    description: "an in-process daemon booted in a #[tokio::test] (current_thread) deadlocks when the test blocks on a child process that calls the daemon; use flavor=multi_thread"
    target: project
    severity: degrading
    workaround: "#[tokio::test(flavor = \"multi_thread\")] in crates/cli/tests/statusline_e2e.rs"
    suggested_encoding: "a one-line note in the testkit/daemon test helpers that boot a daemon in-process"
    fp: "106258a9487a"
    disposition: kept
    system:
      compound:
        status: open
        source: agent-self
        first_seen_at: "2026-10-27T14:11:15.356Z"
  - id: INS-001
    kind: insight
    description: "spawn-rate attribution: proc_pidinfo cannot read pre-exec forks, zombies or root processes; kinfo_proc via sysctl can, and showed the daemon at ~470/s not the brief's 69/s"
    target: project
    fp: "26acf9d5f96e"
    disposition: fixed-now
    system:
      compound:
        status: encoded
        source: agent-self
        first_seen_at: "2026-10-27T14:11:19.917Z"
        resolved_by: "893ae50 (exact per-site launch counter in pij-rs ping) + docs/how/macos-defender.md"
  - id: DL-003
    kind: difficulty
    description: "a scripted edit with an unasserted str.replace silently matched nothing after rustfmt reflowed the code; a mutant survived because a golden case never entered the corpus (only a 'field never read' warning hinted). Recurred twice more later in plan 180."
    target: skill
    severity: degrading
    workaround: "assert match counts on every scripted replace; edit the formatted text with the edit tool"
    suggested_encoding: "agent guidance: scripted source edits must assert their match count (and run after cargo fmt)"
    fp: "3cb24892b08a"
    disposition: kept
    system:
      compound:
        status: open
        source: agent-self
        first_seen_at: "2026-10-27T14:11:20.366Z"
  - id: GFT-001
    kind: gift
    description: "Injection guard: the \\r branch of the control-mode quote/newline refusal had no test, so its mutant SURVIVED; a guard over a character set needs one mutant per member, not one for the set"
    target: project
    fp: "69e0a47a0dbf"
    disposition: fixed-now
    system:
      compound:
        status: encoded
        source: agent-self
        first_seen_at: "2026-10-27T14:42:55.832Z"
        resolved_by: "e6b80e6 (carriage-return assertion; mutant M5r killed)"
  - id: DL-004
    kind: difficulty
    description: "Seat queue starvation again: the prime's ruling (job 4658) sat pending behind 5 stale messages for an hour because pij delivers serially and only between turns; I found it only by reading jobs in sqlite. Wish: pij report/ping shows 'N pending for you, oldest Xm'"
    target: tooling
    severity: degrading
    workaround: "end turns at milestones; sqlite3 -readonly ~/.pij-rs/pij.sqlite jobs where serial_key=<seat> and state=pending"
    suggested_encoding: "pij report now / pij-rs ping / status line show the caller's pending count and oldest age"
    fp: "998bbfc86049"
    disposition: deferred
    system:
      compound:
        status: open
        source: agent-self
        first_seen_at: "2026-10-27T14:42:55.973Z"
  - id: INS-002
    kind: insight
    description: "Bare-name Command::new(\"ps\") on macOS: posix_spawnp creates and tears down a kernel process per PATH miss; with a 32-entry PATH that is 18 process creations per launch. The exact per-site counter (C) plus kinfo over the same window exposed it; neither alone did"
    target: project
    fp: "3864e75dce32"
    disposition: fixed-now
    system:
      compound:
        status: encoded
        source: agent-self
        first_seen_at: "2026-10-27T14:55:40.500Z"
        resolved_by: "0acbf95 (/bin/ps, /bin/date; guard no_daemon_launch_searches_path_for_ps_or_date; decoy-on-PATH test); git/tmux in pij-fleet#74"
system:
  compound:
    bubble_action: "all-save"
---

# Retro — plan 180, pij spawn storm

Narrative, estimate-vs-actual and the four deploy windows: `docs/retros/pij-prime-spawn-storm-plan-180.md`.
Highest-value open entry: DL-004 (a seat cannot see its own pending pij queue), the plan's magic wand.
