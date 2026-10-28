# Plan 183 rulings (prime, 2026-10-28)

- **R1 Inbound first.** Jordan drives pij from his phone through this sidecar; a deaf inbound channel degrades a control. Inbound polling must run every tick, whatever outbound is doing.
- **R2 No real Telegram.** Tests use a fake Telegram server only. Never read `~/.pij/telegram.env` or the daemon's credential file, never send a real message, and never restart the legacy TS telegram bridge. The prime runs the live canary after merge.
- **R3 Migrations.** Avoid a store schema change if existing columns can carry it. If one is needed, tell the prime before writing it; the store is backed up before any migration merges.
- **R4 Daemon.** The machine daemon (127.0.0.1:7461) is read-only for you: never bounce, restart or deploy it, and never run a second daemon on the default tmux server.
- **R5 Ship.** Commit, push the branch, open the PR with `Fixes #77` (standing approval). Do not merge. `git ls-remote origin 'refs/notes/*'` must be empty after every push.
