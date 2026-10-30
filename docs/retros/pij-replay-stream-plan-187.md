# Retro: plan 187, event replay streams in pages (pij-replay-stream)

Coder seat, Claude Opus 5.5, 2026-10-30. Parent: pij-far-jackal (pij-fleet prime). PR #102.

## What shipped

`GET /v1/events` decoded every row after the client's cursor before it sent the hello. From cursor 0 on the live 4.2M-row spine that took 5.6 s, and chainglass recycles every 5 s, so it never saw a frame and its cursor never moved. The replay is now paged (1,000 rows by `seq`): one page before the hello, the next only when the client has taken the last. On a private replay of the live spine's layout, main: 0 frames in 60 s, ~220% CPU, RSS 6 → 13.3 GB. Branch: first frame 1–22 ms, head in 15 s, then 1–2% CPU and 36 MB. The refusal log is now kept per source. The plan's live premise for that change was a misread (see difficulty 1), and the receipt says so.

## magicWand

**A first-class "private daemon on a copy of the live spine's shape" command.** The diagnosis took one seed command and one client script, and both now live in the evidence folder (`private-replay.sh`, `chainglass-like.mjs`). The thing that made the result trustworthy was copying the live spine's `seq/kind/seat/at` with synthetic payloads. A uniform synthetic `bg.lost` fill (plan 185's approach) has no resume-kind gaps, so it can't show the second bar: a 291,544-row stretch the client has to cross inside one connection. Wanted: `just replay-seed` (a read-only ATTACH of the live store, scrubbed payloads, `df` check first) and `just replay-run <bin> <client>`, which reports first frame, cursor progress, CPU and RSS. Plan 185 asked for the same thing (`just cost-replay`). Two plans in two days hand-rolled it.

## Difficulties

| # | severity | what | encoded? |
|---|---|---|---|
| 1 | medium | **A per-minute count was read as a rate.** The plan's "439 lines in 8 minutes" was `uniq -c` over minute 08:11, and those lines came from the pre-bounce daemon. The new daemon had logged once. An AC was built on it ("find why it didn't hold"). | Refuted in diagnosis.md with the boot line and the log's mtime before any code. The prime accepted and kept the per-source refinement. Lesson: when counting log lines across a restart, split on the `listening on` line first. |
| 2 | low | **zsh doesn't word-split `set -- $var`.** My ddocs write loop silently wrote nothing for 6 of 9 rows; the `"status":"error"` lines were the only tell. | Re-ran explicitly. `stretch:` ddocs could refuse an address with a space in its first path segment by name. |
| 3 | low | **Packet says `node_modules/.bin/ddocs`; the worktree has no node_modules** (and the packet forbids installing). The global `ddocs` worked. | Noted. Same family as plan 185/186's deps-less worktree. |
| 4 | low | **Adding a required port method touches crates outside the frozen scope** (testkit fakes/contract, a sidecars test spine). Ten implementors in total, eight of them test wrappers. | Called out in the PR. A required method is still right, for the same reason `newest()` is: a default of `tail()` + truncate would quietly bring the cost back. |
| 5 | low | **RED-on-main for a new port method.** A test that calls a new method can't compile on main. I added each seam (port, `tick_failures`) with main's behaviour first, captured the RED through it, then switched the behaviour. | The receipts say exactly what "main" means for them (`replay-red.txt` header). |
