# Original ask

From the prime (pij-far-jackal), 2026-11-22, Jordan-approved (C35), F280:

> "need to be able to config in pij using configs alternate ways to start a harness e.g. --harness claude (or whatever) --alt 1"
> "also log which alt was used so revive can find it too"

Motivating case: a second Claude Code setup started by a shell function `cc-alt`,
`CLAUDE_CONFIG_DIR="$HOME/.claude-alt" claude --dangerously-skip-permissions "$@"`.
pij can adopt such a seat, but `pij-rs spawn` can't start one, and revive relaunches it as plain
`claude` (wrong home, so `--resume` misses its transcript).

Canary (Jordan: "you can test it with cc-alt"): configure an alt matching `cc-alt`, spawn a throwaway
Claude seat, confirm it binds with its transcript under `~/.claude-alt/projects/`, close, revive,
confirm it resumes the same session in the same home, close.
