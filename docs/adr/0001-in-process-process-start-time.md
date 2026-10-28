# ADR 0001: Read process start times in-process

- **Status:** Accepted (plan 182, pij-fleet#74; ruled by the pij-fleet prime, 2026-10-28)
- **Date:** 2026-10-28

## Context

`ProcLiveness::proc_start` runs `/bin/ps -o lstart= -p PID` for every liveness
check. On the live fleet that is `ps.proc-liveness` at 13.1 launches/s (63 seats
checked every 5 s by the death sweep, plus request paths), which is nearly all of the
daemon's remaining launches (`assets/evidence/before-main.txt`).

The answer is a packed **local wall-clock** time, `YYYYMMDDhhmmss`, and the
registry persists it. A replacement has to give the same answer for every pid:
a live process, a dead pid, a zombie, and another user's process (R1). That means:

1. the start instant `ps` reads (macOS: `sysctl KERN_PROC_PID` → `kinfo_proc.p_starttime`;
   Linux: `/proc/<pid>/stat` field 22 + `/proc/stat` `btime`), and
2. the conversion `ps` applies: libc `localtime()`, using the timezone offset in
   force *at the start instant* (DST included), truncated to whole seconds.

`unsafe_code` is forbidden workspace-wide, so (1) on macOS and (2) on both OSes
need a dependency (R2).

Times below were measured 2026-10-28 on macOS 27.0.1, arm64.

## Candidates for the macOS start time

| crate | licence | releases / downloads | deps (macOS) | build.rs | result |
|---|---|---|---|---|---|
| `sysctl` 0.7.1 | MIT | 2025-09-17; 24.5M | bitflags, byteorder, enum-as-inner, libc, thiserror | none | **Cannot address `kern.proc.pid.N`.** `Ctl::Oid([1,14,1,pid])` runs `oidfmt` first, which returns ENOENT; `Ctl::Name` goes through `sysctlbyname`, which also returns ENOENT. Both were tried on self, pid 1 and a dead pid. |
| `libproc` 0.14.11 | MIT | 2025-10-01; 11.0M | errno, libc | **bindgen** (needs libclang) | Rejected by R2: the build script generates code. It also reads through `proc_pidinfo`, which has the gap below. |
| `sysinfo` 0.39.6 | MIT | 2026-07-09; 233M | libc, memchr, objc2-core-foundation, objc2-io-kit | none | Rejected: the `system` feature pulls in the objc2 tree, which is not minimal. It reads through `proc_pidinfo`, which has the gap below. |
| `darwin-libproc` 0.2.0 | MIT/Apache | 2020-06-17; 1.5M | darwin-libproc-sys, libc, memchr | none | Rejected: unmaintained for six years. |
| `psutil` 5.4.0 | MIT | 2025-07-25; 4.6M | darwin-libproc, mach2, nix, … | none | Rejected: it depends on darwin-libproc and pulls in a large tree. |
| `proc_pidinfo` 0.1.4 | MIT/Apache | 2025-06-03; 86k | libc | none | Gives the exact `p_start` for the caller's own processes. It returns **nothing** for zombies and for other users' processes (pid 1 launchd, mds, WindowServer), where `ps` prints a start time. |

**The `proc_pidinfo` gap, measured.** `PROC_PIDTBSDINFO` returned `None` for a zombie,
for pid 1, for 374 (`mds`, root) and for 418 (`_windowserver`). `ps` printed a
start time for every one of them. So no candidate can stand alone and keep R1:
reading every pid through `proc_pidinfo` would turn a zombie or a recycled-into-root
pid from `Recycled` or `Active` into `Dead`.

**Std-only alternatives.** Neither half is possible on macOS: there is no
procfs, and `std` exposes no `sysctl` or `proc_pidinfo`. Linux can read the
start instant with std alone. Converting it to local time with std alone would
mean writing a TZif/POSIX-TZ parser that matches glibc (TZ env forms, slim
zoneinfo footers), which is more code and more parity risk than calling the
libc function `ps` itself uses.

## Candidates for epoch → local time (both OSes)

| option | what it calls | new crates | parity |
|---|---|---|---|
| `time` 0.3.55 (already in the workspace) with its `local-offset` feature | libc `localtime_r` | `num_threads` 0.1.8 (MIT/Apache, jhpratt; released 2026-10-17, 11 days ago) | Exact. It is the same libc call `ps` uses, including TZ env handling and the offset in force at the start instant. |
| `jiff` / `tz-rs` | Their own TZif parser | 1–2 | Close, but these re-implement libc's rules. |
| A hand-written TZif parser | std only | 0 | The most code and the most parity risk. |

## Thread safety of `local-offset` (verified)

Older `time` 0.3 releases refused `local_offset_at` (`IndeterminateOffset`)
whenever the process had more than one thread. The pinned 0.3.55 does not.
`src/sys/local_offset_at/unix.rs` is `localtime_r` plus `tm_gmtoff`, with no thread
check. `num_threads` now gates only `util::refresh_tz` (`tzset`), at
`src/sys/refresh_tz/unix.rs:47`. Measured inside a tokio `multi_thread` runtime
(8 workers plus 4 extra threads), it gives the offset in force at the instant
asked about: Melbourne 2026-03-31 → +11, 2026-07-01 → +10, now → +11
(`assets/evidence/r2-probes.txt`). Two in-repo tests guard this:
`liveness_parity.rs` and `liveness_launches.rs` probe from a process with extra
threads, so a `time` release that brings the refusal back turns them red.

## Decision

1. **macOS:** `proc_pidinfo =0.1.4` (released 2025-06-03), `PROC_PIDTBSDINFO`,
   `pbi_start_tvsec` as the fast path. The result is accepted only when the full
   struct came back and `pbi_pid == pid`.
   - When it answers nothing, `kill(pid, 0)` (nix) decides whether a process
     exists. `ESRCH` means the kernel has no process with this id, the same fact
     `ps -p` reports as an empty selection, so the answer is "absent" with no
     launch (review of #84, S2). That decision is a **policy** input, not a
     brake: `ESRCH` decides `Dead`. Ids `kill` would read as a process group or
     "every process" (0, and anything above `i32::MAX`) are never sent.
   - Only a process that exists but cannot be read here (a zombie, another
     user's process, any read error) falls back to today's `/bin/ps` probe
     unchanged, so those cases keep today's answer byte for byte. The spawn
     counter records that launch as `ps.proc-liveness-fallback`, so `ping` shows
     how often it fires.
   - Live fleet, 2026-10-28: all 63 seat pids were own-user, live processes, so
     the steady state launches nothing.
2. **Linux:** `/proc/<pid>/stat` field 22 ÷ `USER_HZ` (100) + `/proc/stat` `btime`,
   with std only, exactly as procps computes `lstart`
   (`boot_time() + TICS_BEGAN / Hertz`, procps-ng 4.0.4 `src/ps/output.c:1059`).
   The pid must lead its thread group (`Tgid` in `/proc/<pid>/status`):
   `/proc/<tid>/stat` also resolves for a thread id that no process has, while
   `ps -p` selects processes only (review of #84, N1). A missing `/proc/<pid>`
   is decided by `kill(pid, 0)` as on macOS; anything else falls back to `ps`.
3. **Both:** enable `time`'s `local-offset` feature for the local-time step
   (`UtcOffset::local_offset_at(start)`, which is libc `localtime_r`). It adds
   `num_threads` 0.1.8 (MIT/Apache, jhpratt, released 2026-10-17, 11 days
   before this ADR). **The offset is the one in force at the start instant,
   never the current one.** Across a DST change the current offset would move a
   live seat's identity by an hour, and the reaper would treat the seat as
   recycled. A test pins this under `TZ=Australia/Melbourne`, and the
   current-offset mutant turns it red.
4. `proc_pidinfo` is a macOS-only target dependency, pinned with `=`. `nix`
   (already a workspace dependency) is added to `pij-harnesses` for `kill` and
   to `pij-core` for `access(X_OK)` in the git/tmux PATH search. The
   `crates/testkit/arch-allowlist.toml` rows name each crate and the reason.

## Soundness: `localtime_r` and `setenv`

`localtime_r` reads `TZ` with `getenv`. That is only sound if no thread calls
`setenv` or `unsetenv` at the same time. Both of these hold:

- No workspace code calls `std::env::set_var` or `remove_var` (repo-wide grep
  over `crates/`: 0 matches).
- It could not compile if it tried. The workspace is edition 2024, where
  `set_var`/`remove_var` are `unsafe fn`, and all ten crates inherit
  `unsafe_code = "forbid"`.

What remains is a dependency's own C or Rust code calling `setenv` at runtime.
None of the daemon's dependencies is known to. On macOS the risk does not
arise: `time` treats the macOS environment as thread-safe, citing Apple Libc's
locked `getenv`/`setenv`.

## A timezone change while the daemon runs

`ps` is a fresh process, so it always converts in the current system timezone.
In-process conversion depends on libc:

- **macOS (the deployed platform).** `localtime_r` follows a timezone change
  in-process. Measured by changing `TZ` in a scratch process, the next call
  reflected the new zone without any refresh (`assets/evidence/r2-probes.txt`). The daemon also calls `time::util::refresh_tz()` (`tzset`, sound
  on macOS) before each conversion. So answers track the current zone exactly
  as a fresh `ps` does, and no liveness answer can differ from today's.
- **Linux.** glibc's `localtime_r` loads the zone once per process, and
  `refresh_tz` is refused in a multi-threaded process. After a system timezone
  change, the daemon keeps converting in the old zone until it restarts.
  **That can flip a liveness answer**, in one case. The other `ps` call sites
  (`ps.registration-native-process`, `ps.uds-peer-start`, `ps.tmux-harness-process-tree`)
  stamp a seat registered *after* the change in the new zone. Liveness would
  read that seat in the old zone, see a different start, and report it
  `Recycled`. Today the same event already flips every seat registered *before*
  the change, because `ps` reads them in the new zone. **Accepted:** Linux is not
  a deployed fleet platform, and a host timezone change is rare. The way out is
  the one it has always been: restart the daemon after changing the host
  timezone.

## Consequences

- `ps.proc-liveness` (13.1/s) is gone, and so is the per-sweep cost of a dead
  seat the reaper retains (its process is gone, but its pane is still open and
  not reincarnated). The reviewer found that class costing 0.2 launches/s per
  seat, indefinitely; `kill(pid, 0)` now answers it without a launch. What
  remains is `ps.proc-liveness-fallback`: 0.2 launches/s per retained seat whose
  pid is a **zombie** or now belongs to **another user's** process, and nothing
  else. Both are rare (a zombie is reaped by its parent; a foreign pid reads
  Recycled and is retired). After deploy, the prime's `ping` reading shows the
  rate; a non-zero steady state names a seat stuck in one of those two states.
- Liveness answers are unchanged. The fast path reads the same kernel field
  (`p_start`) that `kinfo_proc` reports, with the same conversion, and every
  other case either gets the kernel's own "no such process" or still runs
  `ps`. Parity tests compare against `/bin/ps` for this process, a live child, a
  zombie, pid 1, a reaped pid and (Linux) a thread id, under the host timezone,
  `America/New_York`, `Australia/Melbourne` and `UTC`.
- `proc_pidinfo` has a single maintainer and little download history. It is
  small enough to audit (756 lines, `libc` only, no build script), it is pinned,
  and the fallback means a broken release costs launches, not wrong answers.
- The other `ps` launch sites (`ps.registration-native-process`,
  `ps.uds-peer-start`, `ps.tmux-harness-process-tree`) are out of scope and left
  for a follow-up.
