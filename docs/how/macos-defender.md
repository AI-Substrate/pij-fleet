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

## What pij does about launches (plan 180)

Exclusions don't stop Defender's EDR inspecting each process launch, so pij now launches far fewer:

- **Daemon tmux reads.** The pane observer used to start three `tmux` clients per tracked pane every 600 ms. The daemon now keeps one tmux control-mode client (`tmux -C`, attached with `no-output,ignore-size,read-only`) and sends its reads (`list-panes`, `capture-pane`, `display-message -p`, `show-options`) over it. Writes still start a tmux client. If the control client is unavailable, slow or lost, the read starts a client as before and the control client is replaced, so answers never change. `tmux list-clients` shows the control client.
- **The Copilot status line.** The managed `~/.pij-rs/copilot-statusline-pij.sh` used to start about 20 processes per refresh (jq, awk, git, a `pij-rs whoami` CLI). It now runs `exec pij-rs statusline copilot`, which is one process with the same output bytes. It asks the daemon for `whoami` on every render, so a seat that dies shows `dead` on the next refresh. It reads the git branch from `.git/HEAD` and runs `git` only for a detached HEAD or an ambiguous name.

## Measure launches

```sh
just spawn-rate --seconds 20          # total new processes/s, and the pij daemon's share
top -l 2 -o cpu -n 12 -stats pid,command,cpu   # Defender: epsext, wdavdaemon_enterprise, wdavdaemon
```

`spawn-rate` counts every launch from the PID counter and attributes them by parent. Short-lived processes exit before a poll sees them, so the per-parent numbers are lower bounds. Compare before and after a change on the same fleet.

To see which daemon code launches what, read the daemon's own counter:

```sh
pij-rs --json ping | jq '.data.spawns'   # {total, by_op: {"ps.proc-liveness": N, "tmux.send-keys": N, ...}}
```

The counts are exact and run since the daemon started, so take two readings a known number of seconds apart and divide the difference. Each operation is named `<program>.<site>`, so `ps.*` names which code runs `ps`. A test (`crates/testkit/tests/spawn_sites.rs`) fails when a daemon launch site doesn't record itself.

## What exclusions don't fix

- **EDR monitoring:** the exclusions stop antivirus file scanning only. Defender's EDR component (`wdavdaemon_enterprise`) still observes every process launch, so the daemon's process launches still cost EDR CPU. Only the device's administrators can tune that.
- **The daemon's own CPU use:** pij-rs's CPU use without scanning is a separate issue, tracked in [pij-fleet#62](https://github.com/AI-Substrate/pij-fleet/issues/62).
