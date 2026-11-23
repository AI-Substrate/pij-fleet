# Change a live Copilot seat's reasoning effort

Relaunch a running Copilot seat on a new `--effort` (for example `medium` → `xhigh`) in the same pane,
resuming the same conversation, with its pij identity and cache-proxy route kept.

> Written in procedure shape (inputs, preconditions, steps, done when, limits) ahead of pij's procedure format,
> which is still being designed. It moves there when that lands. First done by hand on `pij-capable-marten`,
> 2026-11-23, by the prime (pij-kind-parrotfish).

## When to use it, and why it is a hand relaunch

- **Use it** when an operator wants a live Copilot seat on a different effort without losing its context.
- A Copilot seat's effort is the **launch flag** `--effort <level>`, recorded on the seat at spawn. Copilot's
  `/restart` reuses the launch flags, and `pij-rs revive` reuses the recorded effort
  (`crates/daemon/src/http/revive.rs`, `effort: prior.effort`; revive has `--model` but no `--effort`).
- On a cache-proxied seat the `/model` picker is gone (`Unknown command: /model`): the `COPILOT_PROVIDER_*`
  environment points Copilot at a custom provider with a fixed model.
- `/settings effortLevel` changes the user default in `~/.copilot/settings.json` for every new session, and
  the launch flag overrides it anyway. Wrong tool.

**Who runs it:** the operator, or the prime or the seat's parent with the operator's yes. Never the seat
itself: the relaunch ends its own process.

## Inputs

| Input | Where it comes from |
|---|---|
| `SEAT` | the seat id, e.g. `pij-capable-marten` |
| `NEW` | the effort to set. `copilot --help` lists `none`, `minimal`, `low`, `medium`, `high`, `xhigh` and `max` under `--reasoning-effort` (pij launches with its short form, `--effort`); which ones work depends on the model |
| `PANE`, `SESSION`, `FOLDER`, `PID` | the seat's row in `pij list --json` (step 1) |

## Preconditions (check each; stop if one fails)

1. The seat is a **Copilot** seat (`harness` is `copilot`).
2. It is **idle with an empty composer**: `state` is `idle` and the pane's input line is empty. Relaunching
   mid-turn cuts the turn.
3. Its row has a `pane`, a `session` and a `proc.pid` that is running.

## Steps

```bash
SEAT=pij-capable-marten NEW=xhigh
```

1. **Read the seat's row.**
   ```bash
   pij list --json | jq -r --arg s "$SEAT" \
     '.data.seats[] | select(.id==$s) | [.harness,.state,.pane,.session,.folder,.proc.pid,.effort] | @tsv'
   PANE=... SESSION=... FOLDER=... PID=...      # from the line above
   tmux capture-pane -p -t "$PANE" | tail -3    # the composer is empty and the footer shows the current effort
   ```
   `proc.pid` is the native Copilot CLI the seat registered; its parent is the pane's process
   (`pij-rs __spawn-child` after a pij spawn, or the `node …/bin/copilot` launcher after an earlier hand relaunch).

2. **Read its launch flags**, minus the effort and any resume:
   ```bash
   FLAGS=$(/bin/ps -o command= -p "$PID" | cut -d' ' -f2- | sed -E 's/ --(reasoning-)?effort[= ][^ ]+//; s/ --resume[= ][^ ]+//')
   echo "$FLAGS"     # e.g. --yolo --no-ask-user --model claude-opus-5.5 --context long_context
   ```

3. **Read the environment to carry**: every `PIJ_*` except `PIJ_SPAWN_ID` (it belongs to the finished spawn),
   and every `COPILOT_*` (the `COPILOT_PROVIDER_*` set is the cache-proxy route; `COPILOT_AUTO_UPDATE` too).
   ```bash
   ENVS=$(/bin/ps -Eww -o command= -p "$PID" | tr ' ' '\n' | grep -E '^(PIJ_|COPILOT_)' | grep -v '^PIJ_SPAWN_ID=' | tr '\n' ' ')
   echo "$ENVS" | tr ' ' '\n' | sed 's/=.*//'    # print the names only
   ```
   Print names, not values, anywhere a log or transcript might keep them. The proxy route carries no token
   (the sidecar holds it), but other variables might. Values containing spaces don't survive this split: check
   the names list against what you expect and stop if one looks cut.

4. **Relaunch in the same pane, resuming the same session**, with only the effort changed:
   ```bash
   LAUNCHER=$(command -v copilot)     # absolute path to the node launcher, e.g. ~/.npm-global/bin/copilot
   tmux respawn-pane -k -t "$PANE" -c "$FOLDER" \
     "env $ENVS $LAUNCHER $FLAGS --effort $NEW --resume=$SESSION"
   ```
   `respawn-pane -k` ends only that pane's processes. Never `pkill` or kill by name (AGENTS.md, D-C34-3).

5. **Verify.**
   ```bash
   tmux capture-pane -p -t "$PANE" | tail -1      # footer reads "· <NEW>", and the context figure is unchanged
   pij list --json | jq --arg s "$SEAT" '.data.seats[] | select(.id==$s) | {state, proc, extension_build}'
   /bin/ps -o command= -p <new proc.pid> | grep -o -- "--effort [^ ]*"
   ```

## Done when

The footer and `ps` both show the new effort, the context figure matches what it was before (the session
resumed, not restarted), and `pij list` shows the seat on a new `proc.pid` with its extension build (it
re-registers by itself).

## Known limits

- **The recorded effort is unchanged.** `pij list` still shows the old `effort`, so a later `pij-rs revive`
  puts the seat back on it. A `revive --effort` flag would fix that; it is not built (needs the operator's yes).
- **pij records the seat as plain, not proxied.** The new process doesn't descend from the launch the daemon
  stamped, so registration records no `cache_proxy` route and the seat reads plain in `pij directory`,
  `just doctor` and the outage notices (`docs/how/pij-rs-api.md`, F213). Its traffic still goes through the
  proxy, because the `COPILOT_PROVIDER_*` environment came with it; check with `ps -Eww -p <pid>`. Only
  `pij-rs revive` records the route again, and revive resets the effort.
- **No pij wrapper on the pane.** The relaunched process runs without `pij-rs __spawn-child`, so if it exits
  there is no exit-status file or launch log for the post-mortem.

## Common mistakes

- **Dropping the `COPILOT_PROVIDER_*` environment** silently moves the seat off the cache proxy: its next wake
  after five minutes idle rebuilds its whole context.
- **Carrying `PIJ_SPAWN_ID`** ties the new process to a spawn that has already finished.
- **Leaving out `--resume=<session>`** starts a new conversation; the seat loses its context.
- **Relaunching mid-turn** cuts the turn. Wait for idle and an empty composer.
- **Using `/settings effortLevel` or `/restart`**: the first changes the default for every new session, the
  second keeps the old flag.
