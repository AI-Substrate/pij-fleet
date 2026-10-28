# pij-launch-trim: daemon launches, in-process liveness and resolve-once (plan 182)

**magicWand:** a probe kit for the dependency ADR. Every R2 candidate needed the
same throwaway crate to answer "does this crate's call answer for self, a child,
a zombie, pid 1, another user's process, and a dead pid, and what does `ps` say
for the same pids?". Building that harness by hand took more of the session than
the liveness code did. `harness probe-crate <crate>@<ver> --pids self,child,zombie,1,dead`
writing a receipt would turn R2 from an afternoon into a command.

**What shipped:**
- Liveness reads a live process's start time in-process: `proc_pidinfo` on
  macOS, `/proc` with `btime` on Linux. It converts with libc `localtime_r`, using
  the offset at the start instant. Any miss runs the unchanged `/bin/ps` probe,
  counted as `ps.proc-liveness-fallback`. The 13.1/s of `ps.proc-liveness` goes
  to ~0.
- `git` and `tmux` are resolved through PATH once (`pij_core::program::resolve`).
  Only hits are kept, a vanished path is searched again, and the first match wins.
- ADR 0001 records why five crates were rejected and the one accepted.

**difficulties:**
- **The obvious crates give the wrong answer.** Every `proc_pidinfo`-based
  crate returns *nothing* for zombies and other users' processes, where `ps`
  prints a time, and the obvious use reads that as "dead". Only running the same
  pids through both showed it. `sysctl` 0.7.1 cannot address `kern.proc.pid.N`
  at all (it calls `oidfmt` first and gets ENOENT). Severity: high. Each would
  have shipped green against tests of the caller's own process only.
- **The prime's memory of `time` was out of date.** Older 0.3 releases refused
  `local_offset_at` when multi-threaded; 0.3.55 doesn't. Testing it inside a
  multi-thread runtime settled the question in one command. Encoded: the parity
  tests probe from a multi-threaded process, so the old behaviour returning would
  turn them red.
- **A guard that still passes but no longer tests anything.**
  `path_search.rs`'s "liveness runs /bin/ps, not a decoy" probed this process.
  After the change it never reached `ps`, so it went on passing without testing
  anything. `first_light.rs` used the same probe as its "known launch". Both now
  probe a reaped pid, and mutant M6 (bare `ps` in the fallback) proves the first
  still bites.
- **A mutant that survived for the wrong reason.** "Cache a miss" as a one-line
  insert survived, because the vanish check already rejects a relative cached
  path. That makes it an equivalent mutant, not a miss cache. A real negative
  cache turns the test red. Write the mutant you mean, not the one that is
  easiest to type.
- **`cargo test` stops at the first failing test binary.** The stale `arch_drift`
  fixtures stayed hidden for two full runs, behind an earlier red binary. Use `--no-fail-fast` before calling a workspace green.
  `stretch:` `just rust-check` should pass `--no-fail-fast`.
- **The workspace has no node_modules**, so `harness checks` cannot run its TS
  stages here, and the packet forbids installing them. NOT RUN, and CI covers
  them. This happens in every Rust-only worktree; `stretch:` `harness checks
  --rust-only` that says so instead of failing five stages.
