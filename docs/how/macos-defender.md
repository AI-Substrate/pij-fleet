# pij-rs and Microsoft Defender on macOS

On a Mac with Microsoft Defender for Endpoint, the `pij-rs daemon` can be the largest source of real-time scans. Defender checks every program launch, and the daemon launches many short-lived child processes (tmux and process probes). The scans add CPU load and can show up as system-wide input stutter.

## Measure it

```sh
mdatp diagnostic real-time-protection-statistics --output json \
  | jq -r '.counters | sort_by(-(.totalFilesScanned|tonumber)) | .[:10][] | "\(.totalFilesScanned)\t\(.id)"'
```

Run it twice a minute apart and compare the counts. On 2026-10-19 one dev Mac measured about 13,700 scans from pij-rs in a few minutes (about 75 a second, roughly five times every other process combined). With the exclusions below in place, it measured 5 in the next 60 seconds. Those numbers are a snapshot; re-measure on your own machine.

## Exclude the daemon

Exclusions usually need admin rights, or your IT department if the device is managed:

```sh
mdatp exclusion folder add --path "$HOME/.pij-rs"
mdatp exclusion process add --path "$HOME/.npm-global/bin/pij-rs"
mdatp exclusion list    # verify
```

- **The binary path:** the process exclusion matches one binary path. `just rs-install` links `$(npm prefix -g)/bin/pij-rs` to `target/release/pij-rs` in the main checkout (`readlink -f "$(command -v pij-rs)"`). If your npm prefix or checkout moves, or the scan counts climb again, add the new path, including the resolved `target/release/pij-rs` path.
- **The state directory:** the folder exclusion assumes the default `~/.pij-rs`. If you run the daemon with another `--state-dir`, exclude that directory instead.

## What pij does about launches (plans 180 and 182)

Exclusions don't stop Defender's EDR inspecting each process launch, so pij now launches far fewer:

- **Daemon tmux reads.** The pane observer used to start three `tmux` clients per tracked pane every 600 ms. The daemon now keeps one tmux control-mode client (`tmux -C`) and sends its reads (`list-panes`, `capture-pane`, `display-message -p`, `show-options`) over it. Writes still start a tmux client. If the control client is unavailable, slow or lost, the read starts a client as before and the control client is replaced, so answers never change. `tmux list-clients` shows the control client.
  - **What stops it writing.** Only pij's own code: an allow-list of those four reads (every other command starts a client instead) and single-quoting of every argument (an argument that can't be quoted also starts a client). The `read-only` attach flag only refuses keyboard input; commands sent over the control pipe run with full rights.
  - **Its own session, `_pij`.** The client attaches to a detached session named `_pij` (one window running `sleep`), which it creates if absent. While `_pij` holds only that window, it is destroyed when no pij client is attached. The client never starts a tmux server, and it leaves when `_pij` is the only session, so the server still exits when you close your last session. pij never lists the control pane itself. Expect one `_pij` row in session pickers and `tmux ls`. A bare `tmux attach` behaves as without pij. `pij spawn --session _pij` is refused (`E-RS-CONTROL-SESSION`).
  - **Known effect: untargeted commands from outside tmux.** A tmux command run from outside tmux with no `-t` (for example `tmux new-window` from a script) goes to the most recently active session. Attaching a client counts as activity, so after the control client starts or reconnects, such a command lands in `_pij`. That lasts until a keypress in another session makes it more recent, and with no human attached it can last indefinitely. A window or pane that lands in `_pij` stays: pij lists it like any other, and `_pij` is then kept even after the control client leaves. Pass `-t` to target a session. pij's own writes always target.
- **The Copilot status line.** The managed `~/.pij-rs/copilot-statusline-pij.sh` used to start about 20 processes per refresh (jq, awk, git, a `pij-rs whoami` CLI). It now runs `exec pij-rs statusline copilot`, which is one process with the same output bytes. It asks the daemon for `whoami` on every render, so a seat that dies shows `dead` on the next refresh. It reads the git branch from `.git/HEAD` and runs `git` only for a detached HEAD or an ambiguous name.
- **Process probes by absolute path.** On macOS, launching a program by bare name (`ps`) makes `posix_spawnp` try every PATH entry, and each miss creates and tears down a kernel process: with a 32-entry PATH, one `ps` cost 18 process creations. The daemon now runs `/bin/ps` and `/bin/date`, which cost one each. A test (`no_daemon_launch_searches_path_for_ps_or_date`) refuses a bare `ps` or `date` in the daemon crates.
- **`git` and `tmux`, resolved once (plan 182).** These live wherever you installed them, so a fixed path would be wrong. The daemon searches PATH for each the first time it launches it, the same way `posix_spawnp` does (the first match that `access(X_OK)` allows wins), and then launches the absolute path it found (`pij_core::program::launch`). It keeps only a found path, so a tool installed after the daemon started is found on its next launch. If a kept path has vanished (an upgrade moved it), it searches again, before the launch or when the launch itself fails with `ENOENT`. A test (`no_daemon_launch_searches_path_for_git_or_tmux`) refuses a bare `git` or `tmux` launch in the daemon crates.
- **Liveness without `ps` (plan 182).** Every liveness check (the 5 s death sweep over every live seat, `whoami`, delivery) used to run `/bin/ps -o lstart= -p PID`. On the live fleet that was `ps.proc-liveness` at 13.1 launches/s with 63 seats, nearly all of the daemon's launches. The daemon now reads the start time in-process: `proc_pidinfo` on macOS, `/proc/<pid>/stat` with `btime` on Linux. It converts that time with libc `localtime_r`, the same conversion `ps` applies, using the offset in force when the process started. A live seat therefore costs nothing. A pid with no process is answered in-process too (`kill(pid, 0)` returns `ESRCH`, the fact `ps` reports as an empty selection), so a dead seat the reaper keeps because its pane is still open costs nothing either. Only a process that exists but can't be read in-process (a zombie, another user's process) runs `/bin/ps` exactly as before, so every answer is unchanged. `ping` counts those launches as `ps.proc-liveness-fallback`; in steady state expect 0, and a non-zero rate names a seat stuck as a zombie or on a recycled foreign pid. The choice of dependency is recorded in [ADR 0001](../adr/0001-in-process-process-start-time.md).

## Measure launches

```sh
just spawn-rate --seconds 20          # total new processes/s, and the pij daemon's share
top -l 2 -o cpu -n 12 -stats pid,command,cpu   # Defender: epsext, wdavdaemon_enterprise, wdavdaemon
```

`spawn-rate` counts every launch from the PID counter and attributes them by parent. Short-lived processes exit before a poll sees them, so the per-parent numbers are lower bounds. Compare before and after a change on the same fleet.

To see which daemon code launches what, read the daemon's own counter:

```sh
pij-rs --json ping | jq '.data.spawns'   # {total, by_op: {"ps.proc-liveness-fallback": N, "tmux.send-keys": N, ...}}
```

The counts are exact and run since the daemon started, so take two readings a known number of seconds apart and divide the difference. Each operation is named `<program>.<site>`, so `ps.*` names which code runs `ps`. A test (`crates/testkit/tests/spawn_sites.rs`) fails when a daemon launch site doesn't record itself.

## What exclusions don't fix

- **EDR monitoring:** the exclusions stop antivirus file scanning only. Defender's EDR component (`wdavdaemon_enterprise`) still observes every process launch, so the daemon's process launches still cost EDR CPU. Only the device's administrators can tune that.
- **The daemon's own CPU use:** pij-rs's CPU use without scanning is a separate issue, tracked in [pij-fleet#62](https://github.com/AI-Substrate/pij-fleet/issues/62).
- **`open()` is not free either:** Defender inspects file opens as well as launches. Plan 185 measured single opens of a daemon state file at 3–138 ms. So a loop that opens files every tick costs CPU here even when it launches nothing. [daemon-cost.md](daemon-cost.md) lists what the daemon still opens, and how to measure it.
