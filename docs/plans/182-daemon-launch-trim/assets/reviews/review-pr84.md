# Review: PR #84 (plan 182, in-process liveness and resolve-once)

Reviewer: `pij-review-pr84`. Head: `ce27a6a`. Diff: `origin/main...HEAD`. Read-only review: nothing was committed or pushed, and the worktree is clean (`git status --short` shows nothing).

**Verdict: merge after fixes.** 0 blockers, 2 should-fix, 4 nits. The liveness semantics are correct. I proved parity against `/bin/ps` across a real DST transition, and both key mutants turn the suite red.

## Proved myself

**1. DST, end to end, with a live process against `/bin/ps`.** I started `/bin/sleep 600`. I then built a POSIX `TZ` whose daylight time (+11, BBB) ends 8 s later (`AAA-10BBB,J1/0,J301/20:12:44`), waited until the clock was past the change, and probed from a 5-thread process. The probe is a scratch crate that uses `pij-harnesses` by path.
```
before transition: ps → Wed Oct 28 20:12:35 2026 ; date → 20:12:36 BBB
after transition:  date → 19:12:46 AAA
ps:   Wed Oct 28 20:12:35 2026
pid 16716: in-process=Ok(Some(20261028201235))      ← matches ps
UTC ps: Wed Oct 28 09:12:35 2026
```
**Mutant** (`local_offset_at(instant)` → `current_local_offset()`), same pid and same TZ:
```
MUTANT probe: in-process=Ok(Some(20261028191235))   ps: Wed Oct 28 20:12:35 2026
proc::tests::local_lstart_uses_the_offset_at_the_start_instant --- FAILED
```
This is the reaper hazard: the mutant moves a live seat's start by one hour, so the seat reads as Recycled. The in-repo test catches it.

**2. The fallback goes to `ps`, never straight to "dead".** Mutant: `return Ok(None)` when the in-process read has no answer.
```
liveness_parity: assertion failed: a zombie (pid 26568) differs from /bin/ps   (also the TZ re-run: 1 passed; 2 failed)
liveness_launches.rs:74 panicked (zombie must still have a start time)
```
Both turn red. The source file was restored afterwards and `git status` is clean.

**3. Baseline is green.** I ran `cargo test -p pij-harnesses -p pij-core -p pij-tmux -p pij-testkit` and every binary reported `ok`, including `liveness_parity` (3 passed), `liveness_launches` and `spawn_sites`. PR CI is green on ubuntu (`rust-test` runs nextest across the whole workspace), so Linux btime parity (`/proc` against procps) has passed at least once on a real Linux.

## Verified correct (by reading the code, with citations)

- **Each kind of pid gets the same answer as before.** Own live processes are answered in-process. Zombies, other users' processes and pid 1 get `Err` or `None` from `PROC_PIDTBSDINFO`, which falls back to the unchanged `/bin/ps` path. Reaped pids also fall back to `ps`. `pbi_pid == pid` is checked, and `proc_pidinfo` 0.1.4 rejects a short struct (`darwin.rs:326`, `res != buffersize`). A recycled pid reads the current process's `p_start`, exactly as `ps` does.
- **Precision.** Both paths use whole seconds from the same kernel field (`p_start.tv_sec`). On Linux the code uses integer `ticks/100`, as procps does.
- **Upgrade transition.** Identities stored by the ps-era daemon compare equal on macOS, because it is the same field and the same `localtime_r` conversion. So no live seat flips when the new build deploys.
- **Thread safety.** In `time` 0.3.55, `local_offset_at` is plain `localtime_r` with no thread gate (`sys/local_offset_at/unix.rs`). `refresh_tz` (tzset) is gated by `OS_HAS_THREAD_SAFE_ENVIRONMENT`, which is true on macOS. The workspace has zero `set_var`/`remove_var` calls (grep over `crates/`), and `unsafe_code = "forbid"` is at `Cargo.toml:106`.
- **Dependency.** `proc_pidinfo` is target-gated to macOS and pinned `=0.1.4`. It has no `build.rs`, depends only on libc and has 7 `unsafe` sites. The arch-allowlist row names both new crates. The ADR's statement of the Linux timezone-change hazard is accurate.
- **Resolve-once (`crates/core/src/program.rs`).** The first PATH match wins and only hits are kept. One mutex covers both the lookup and the search, so concurrent resolves are serialized and correct. The kept path is the PATH entry (for example the Homebrew symlink), not its canonical target, so a brew upgrade or an in-place rename stays valid. A kept path that has vanished is searched for again.
- **tmux crate.** The only behaviour change is `Command::new(resolve(&self.binary))` at `lib.rs:218` and `control.rs:285`, plus a `#[cfg(test)]` accessor and a test module. Nothing matches the tmux process by argv, so argv[0] becoming an absolute path is harmless. The new `#[cfg(test)] mod resolve_once` (lib.rs:1405) sits just above the existing first test module (1458), so no production lines moved past `production_lines`' cut.
- **Cost.** The PR adds no new per-tick launch. The steady state is about 12.6 `proc_pidinfo` calls per second plus one `tzset` per probe, all in-process.

## Should-fix

**S1. The `spawn_sites` `#[cfg(test)]` skip can hide a production launch.** (`crates/testkit/tests/spawn_sites.rs:118-130`)
A "function" is everything from one `fn` line that `fn_name` recognizes to the next. `fn_name` does not recognize `pub(in …)`, `pub(self)` or `extern "C" fn`. Such a function's body therefore merges into the previous segment, and if that previous function is `#[cfg(test)]`, the new skip drops the whole segment. Before this PR the launch was at least attributed to the previous function, so it failed loud. I injected the following into `daemon/src/orchestration/repo.rs`:
```
#[cfg(test)] fn test_accessor() {}
pub(in crate::orchestration) fn launch_unrecorded() { let _ = Command::new("/bin/ls").output(); }
```
```
CONTROL (no cfg(test)):  repo.rs::test_accessor … test result: FAILED. 2 passed; 1 failed
HIDDEN  (cfg(test)):     test result: ok. 3 passed; 0 failed
```
No such function exists today (`grep 'pub\((in |self)' … fn` matches nothing), so this is latent. Fix: have `fn_name` strip any `pub(…)` prefix and `extern "…"`, or end a skipped segment at the function's closing brace.

**S2. The ADR's cost statement for the fallback is wrong for one steady-state class.** (`docs/adr/0001…` § Consequences: "about one launch per seat death")
The reaper keeps a seat whose process is dead but whose pane is present and not reincarnated (`reaper.rs:73-85`: `Dead` → pane present → `reincarnated == false` → `Retain`). That seat stays active, so every 5 s death sweep probes it again, and with no in-process answer it launches `ps` each time. That is **0.2 launches/s per such seat, indefinitely**: the "per tick × per seat" shape AGENTS.md § Resource cost calls out. A typical case is a harness that exited back to its shell. This is not a regression, since main launches `ps` for every seat, but the plan's backpressure row should state it. After deploy, confirm the `ps.proc-liveness-fallback` rate from `ping`. If it is non-zero in steady state, either retire these seats or cache a "dead" verdict for a pid that cannot come back alive. A dead pid can only become a different process, which reads as Recycled, so such a cache would be a brake rather than a policy.

## Nits

- **N1 (PLAUSIBLE, Linux only, not run).** `/proc/<tid>/stat` resolves for a thread id even though it is not listed. If a recorded pid is recycled as a thread id of some other process, the in-process read returns the thread's start time and the seat reads Recycled, whereas procps `ps -p` selects by process id and reports Dead. Neither answer is Active, but the reaper's reason and process string differ (`reaper.rs:76`). Fix: check `Tgid == pid` in `/proc/<pid>/status`, or fall back to `ps` when they differ.
- **N2.** `program::executable` tests the mode bits (`& 0o111`) rather than `access(X_OK)`. A file executable only by another user or group is chosen, whereas `posix_spawnp` would get EACCES and move on to the next PATH entry. In addition, the vanish check runs a `stat` before the launch rather than retrying on launch failure (R3 says "when launched"), so if the binary disappears between the `stat` and the spawn, that one launch fails.
- **N3.** `no_daemon_launch_searches_path_for_git_or_tmux` matches literal strings. `Command::new(binary)`, `Command::new(TMUX)` and `Command::new(OsStr::new("git"))` all pass it. It is a tripwire, not a proof.
- **N4 (follow-up, out of scope).** Other bare-name launches remain in daemon crates: `http/identity.rs:569` (`lsof`, on the request path) and `harnesses/models.rs:325` (`omp`). Each one still pays the PATH-miss processes.

## Not run, and why

- `harness checks` and full-workspace `just rust-check`: heavy, and CI already covers fmt, clippy and nextest. I ran the four affected crates instead.
- `crates/daemon/tests/first_light.rs`: it starts a daemon, and the packet forbids running a second daemon near the machine fleet. CI ran it.
- A system timezone change (`/etc/localtime`) while the daemon runs: this needs sudo. Only the `TZ` env path is proven, both by me and in the ADR.
- Anything on Linux locally, so N1 is reasoned, not run.
- Live before and after `ping` readings: R4 assigns those to the prime after deploy.
