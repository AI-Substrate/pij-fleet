# Copilot seats show ctx "?" in the nudge; read size from the pane footer

tags: pa, watchdog, context, copilot

Hook: the watchdog digest has no context size for Copilot seats; the pane footer line with ⛓ has it.

Observed 2026-11-04 14:55 round: every community seat except the prime showed ctx `?` and `pij-rs list --json` had no sessionStatus/contextUsed for them. The Copilot footer line (`… ⛓ <seat> • <model> … • 515k/1.0M • …`) gives the size:
`tmux capture-pane -p -t <pane> | grep '⛓' | tail -1`. Panes come from `pij-rs list --json` (`data.seats[].pane`). Use this only for the 600k context-keeping check.

That round: careful-heron 515k, brisk-marten 522k, steady-otter 304k, swift-kestrel 73k. Watch heron and marten next.

A "[pij native] unavailable: reconnecting" banner on every pane right after a daemon restart is expected; the extension retries.
