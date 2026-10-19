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

## What exclusions don't fix

- **EDR monitoring:** the exclusions stop antivirus file scanning only. Defender's EDR component (`wdavdaemon_enterprise`) still observes every process launch, so the daemon's process launches still cost EDR CPU. Only the device's administrators can tune that.
- **The daemon's own CPU use:** pij-rs's CPU use without scanning is a separate issue, tracked in [pij-fleet#62](https://github.com/AI-Substrate/pij-fleet/issues/62).
