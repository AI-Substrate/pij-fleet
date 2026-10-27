# Plan 181 diagnosis: Codex bind and identity

Seat `pij-codex-bind`, 2026-10-28. Codex CLI 0.164.0 (standalone), Codex app-server
daemon 0.165.0 (self-updated 2026-10-28 06:35). Machine daemon: main `26fc3ba` for
canaries A and B; rebuilt and bounced by someone else to `077d383` (PR #71, telegram
sidecar only, no change to spawn, registration or identity) at 07:12:07, before canary C.

Raw evidence: `canaries/<label>/` (spawn command and output, pane captures at 0 s, at
return and settled, store row, events, the seat's own probe output). Scripts:
`canaries/canary.sh`, `canaries/probe.sh`, `canaries/codex-wrapper-{B,C}.sh`.

## Summary

| # | Defect | Reproduced | Root cause | New? | Plan 177 |
|---|---|---|---|---|---|
| 1 | Spawn times out waiting for registration | yes, 3 of 3 | pij has no registration path for a spawned Codex seat | no: same failure on 2026-09-08 (Codex 0.153) | no effect |
| 2 | `pij send` from a Codex seat arrives as another seat | yes, canary B | Codex's shared app-server daemon runs every thread's tool shell with the environment of the TUI that started it; pij takes identity from that environment | Codex-side change, first seen 2026-09-26 (0.157) | no effect |
| 3 | `⚠ … · /status` read as a footer (ac-0004) | by code reading | `parse_codex_footer` accepts any `<word> <word> · … · /path` line | n/a | introduced the current parser, not this gap |

Cause 2 lies in Codex behaviour (R2), so it is reported to the prime before any fix.

## Defect 1: spawn never registers

**Observed.** Canary A, exact command in `canaries/A-untrusted/spawn-cmd.txt`:
`pij-rs spawn --harness codex --model gpt-6.1-sol --effort low --role worker --name c181-A-untrusted --cwd <this worktree> --wait-seconds 60 --json`
returned `timed out after 60s waiting for registration` for `pij-poised-anteater` in
`%113`. The pane was healthy from 0 s: `pane-t000.txt` already shows
`GPT-6.1-Sol low · ~/pi-hacking/fleet-181-codex-bind` and `? for shortcuts`. B and C
failed the same way at 30 s.

**Events.** The seat's whole event history is `seat.put` (proc null), `role-set` and
`spawn.failed` (`canaries/*/events.txt`). Nothing ever attempted to register it.

**Cause.** The spawn waiter (`crates/daemon/src/http/mod.rs:1385`) succeeds only when
the row gains a `proc`. The spawned row starts with `proc` absent "until the new
incarnation registers" (`mod.rs:1316`). For Claude, the daemon's `ClaudeBinder`
(`crates/daemon/src/claude_bind.rs:73`, filter `is_pending_spawned_claude`) binds that
row from `~/.claude/sessions/<pid>.json`. Copilot, OMP and Pi register from their own
extensions. Codex has no binder, no hook (`~/.codex/config.toml` hooks are git-ai
only) and no extension. A spawned Codex seat binds only if its agent runs `pij adopt`
itself, and pre-bind delivery is held (`delivery.held reason:"pre-bind"` in the osk
history), so nothing tells it to.

**Not new.** `pij-grumpy-sniper` (2026-09-08, Codex 0.153.4, pane %4409):
`spawn.failed … timed out after 30s waiting for registration` at 15:59:35, then bound
at 16:03:11 only after the seat ran adopt by hand (store events seq 22944-22954).
Plan 177 changed footer parsing only; the footer in every capture here parses
correctly, and nothing consults it on this path because no binder exists.

**Folder trust: not reproduced.** Canary A ran in this worktree, which is absent from
`~/.codex/config.toml` `[projects]`; no trust prompt appeared
(`--dangerously-bypass-approvals-and-sandbox` shows `permissions: YOLO mode`).

**A dialog does block.** In B and C an `Update available · 0.164.0 → 0.165.0` menu
(`› 1. Update now … 2. Skip … enter continue · esc skip`) covered the composer, and
the spawn still reported only `timed out` (`canaries/B-daemon/spawn-out.json`).
ac-0002's named error is needed for this dialog, not for trust.

## Defect 2: replies attributed to another seat

**Codex runs TUI threads in a shared daemon.** Since 0.157 the Codex TUI auto-starts
(`codex features list`: `daemon_auto_start  stable  true`) and connects to one
machine-wide app-server daemon:

- Rollout `session_meta.source` is `"vscode"` for daemon-hosted TUI threads and
  `"cli"` for in-process ones. Codex TUI rollouts on this machine: `cli` through
  0.153 (2026-09-08), first `vscode` 2026-09-26 (0.157.1), all 14 on 2026-10-26 `vscode`.
- `~/.codex/logs_2.sqlite`: noisy-lizard's `thread/start` at 2026-10-26 21:45:03 used
  `rpc.transport="unix_socket"`; canary A's at 2026-10-28 07:03:38 used `"in-process"`.

**The daemon carries the first launcher's environment.** The daemon (`40583`,
`codex app-server daemon pid-update-loop`, parent 1, started 2026-10-26 19:29:40) and
its server (`28580`, respawned by Codex's own 0.165 update at 06:35 today) both have,
per `ps eww`:

```
PIJ_SESSION_ID=pij-parallel-bandicoot
PIJ_SPAWN_ID=s-824372647206abac19f89dbfb5b35c3d
TMUX_PANE=%45
```

`pij-parallel-bandicoot` was the first Codex seat spawned that evening (19:29:39,
pane %45). Its `spawn.failed` capture shows Codex logging `process_spawned` from its
TUI at that moment. The pane is gone and the seat was closed at 19:51. The env still
lives in the daemon.

**Every daemon-hosted thread's tool shell inherits it.** Canary B (`--bin` wrapper
`codex --remote unix://`, i.e. daemon-hosted, the mode of 2026-10-26; its hint line
reads `← for agents · ? for shortcuts`, as in the osk report) ran
`canaries/probe.sh` from its own tool shell (`canaries/B-daemon/probe.txt`):

```
CODEX_THREAD_ID=01a17f33-fede-7c92-afba-b421d49c9a9b   <- its own thread: correct
PIJ_SESSION_ID=pij-parallel-bandicoot                   <- another seat
TMUX_PANE=%45                                           <- another seat's dead pane
 7102 28580 … /bin/sh …/probe.sh B-daemon nosend        <- parent is the daemon, not the TUI in %114
whoami: refused "no live seat in the rs store owns pane `%45` … Run `pij adopt "$TMUX_PANE" …` first."
```

Canary A (in-process, same cwd, at the same time) shows its own `PIJ_SESSION_ID` and
`TMUX_PANE=%113`, a parent of its own TUI, and its `pij send` arrived as
`pij-poised-anteater` (`canaries/A-untrusted/probe.txt`). So the shared cwd is not the
factor; the hosting mode is.

**How that produced each osk report.** The shim sends `PIJ_SESSION_ID` and
`TMUX_PANE` as caller context (`.omp/extensions/pij/core/generation-routing.ts:363`),
and the daemon resolves the sender from them (`crates/daemon/src/http/identity.rs:1189`
`resolve_seat`).

- While bandicoot lived (19:29-19:51), every daemon-hosted Codex seat resolved as
  bandicoot. Store `message.pushed` from `pij-parallel-bandicoot` includes bodies
  `db-docs re-review (pij-wild-guan)`, `ib-05 re-review (pij-stellar-butterfly)` and
  `wave-c infra review (pij-wild-guan)`.
- After bandicoot closed, noisy-lizard (pane %53, rollout `01a17807…`) got
  `no live seat … owns pane %45`, followed the advice, and ran
  `pij adopt "$TMUX_PANE" --harness codex` with `TMUX_PANE=%45`. The daemon bound pane
  %45's still-running Codex process (pid 39324) as a NEW seat `pij-powerful-lobster`
  with `parent: null`, `session` = noisy-lizard's own thread. Then
  `env -u PIJ_SESSION_ID pij send …` delivered `r35 review: REJECT` as powerful-lobster
  (rollout tool calls 11:48:30-11:48:53Z; store seq 884093, 884162).

**Lever (diagnostic, not yet a fix).** Canary C passed the seat's own values as
per-thread overrides:
`-c shell_environment_policy.set.PIJ_SESSION_ID="$PIJ_SESSION_ID"` (and `PIJ_SPAWN_ID`,
`TMUX_PANE`). Still daemon-hosted (parent `28580`), its shell showed
`PIJ_SESSION_ID=pij-steep-lobster`, `TMUX_PANE=%116`, and `pij whoami` returned
`pij-steep-lobster` (`canaries/C-daemon-envset/probe.txt`). Per-thread config beats
the inherited daemon env.

**Not reproduced: why canary A ran in-process today** with `daemon_auto_start` still
true. The daemon is 0.165 and the TUI 0.164; a version mismatch is the likely reason
but is not proven, and it is Codex-internal. Pane-id reuse after the 2026-10-26 reboot
is not the cause: the stale `%45` was minted after the reboot, and what outlived it was
the Codex daemon.

## Defect 3: warning line read as a footer

`parse_codex_footer` (`crates/harnesses/src/composer.rs:215`) takes the first word as
the model, needs any second word, and accepts the first later `·` segment that starts
with `/`. `⚠ 5h limit: 49% left · resets at 12:33 AM on 27 Oct · /status` (noisy-lizard
`spawn.failed` capture) parses as model `⚠`, cwd `/status`. In that capture the real
footer sits below the warning, and `observed_model`/`observed_cwd` search bottom-up
(`adapter.rs:349,367`), so the real footer wins. That is why it was not the bind cause.
`classify_readiness` uses `any` (`adapter.rs:278`), so a frame showing only the
warning line would still read Ready. The hint line `← for agents · ? for shortcuts …
⚠ 2 warnings · f2 to view` has no `/` segment and is already rejected.

## Canaries (R3)

| label | seat | pane | spawn | mode | torn down |
|---|---|---|---|---|---|
| A-untrusted | pij-poised-anteater | %113 | timed out 60 s | in-process | see below |
| B-daemon | pij-compatible-emu | %114 | timed out 30 s (update dialog) | daemon (`--remote unix://`) | see below |
| C-daemon-envset | pij-steep-lobster | %116 | timed out 30 s | daemon + per-thread env | see below |

One further attempt at 07:12:06 never reached the daemon (bounced mid-request); no
seat or pane was created.
