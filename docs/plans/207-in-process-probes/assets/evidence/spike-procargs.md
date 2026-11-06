# Plan 207 spike: macOS process facts in-process without `unsafe`

tk-0002, pij-following-marsupial, 2026-11-06. Probes ran as a normal user in a scratch test (not committed), through `just shared-build` on this worktree's target.

What registration's ancestry walk reads per ancestor, today with `/bin/ps -ww -p <pid> -o ppid= -o ucomm= -o args=`: the parent pid, the kernel accounting name (`ucomm`, `p_comm`) and argv.

| Read | Source tried | pid 1 (root, launchd) | this process | its parent |
|---|---|---|---|---|
| parent and `ucomm` | `proc_pidinfo` 0.1.4 `PROC_PIDT_SHORTBSDINFO` (already a dependency) | ppid 0, `launchd` | ppid 24322, `zz_probe_207-97` | ppid 24302, `cargo` |
| argv | `sysctl` 0.7.1, `Ctl::Oid([CTL_KERN, KERN_PROCARGS2, pid])` | ENOENT | ENOENT | ENOENT |
| argv | `sysctl` 0.7.1, `Ctl::Name("kern.procargs2.<pid>")` | ENOENT | ENOENT | ENOENT |
| `kinfo_proc` | `sysctl` 0.7.1, `[CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]` | ENOENT | ENOENT | ENOENT |

`ps` printed `0 launchd /sbin/launchd` for pid 1, so the short BSD info's `launchd` matches `ucomm`.

Findings:

- **The parent and name come from `proc_pidinfo`'s short BSD info, for every process, root-owned ones included.** No new crate is needed for that part.
- **`sysctl` 0.7.1 cannot read pid-suffixed OIDs.** Its value path first asks the kernel for the OID's type (`{0,4}` + oid). For `kern.procargs2.<pid>` that lookup returns ENOENT, so it never reads the value. Rejected.
- **`sysinfo` is rejected without probing.** Its macOS `refresh_processes_specifics` calls `proc_listallpids` and walks every pid even for `ProcessesToUpdate::Some` (`src/unix/apple/system.rs:236-300` in 0.39.6). That is a machine-wide census, which the registration comment rules out ("One requested PID, never a machine-wide process census"). Its `system` feature also brings the objc2 CoreFoundation and IOKit crates into the daemon.
- **argv needs `sysctl(KERN_PROCARGS2)`.** That is the same call `ps` makes, and calling it directly needs `unsafe`, which the workspace forbids (`Cargo.toml` `unsafe_code = "forbid"`). The choice went to pm-001 and the prime.

## Parent and name, kept

`pij_harnesses::proc::parent_and_name` (macOS) reads the short BSD info. `proc::tests::parent_and_name_match_ps_without_a_launch` checks it against `ps -o ppid= -o ucomm=` for pid 1 and this process, with no launch, and checks that a reaped pid reads as `None`. One measured difference: the short info's name field is 16 bytes including the NUL, so a 16-character `p_comm` reads as its first 15 characters. The test binary `pij_harnesses-c8…` came back as `pij_harnesses-c`. Registration compares this name only with `copilot`, `claude`, `codex` and `node`, so its results do not change.
