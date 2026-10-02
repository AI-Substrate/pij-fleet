# `harness boot` — agent briefing

> Served verbatim by `harness instructions boot`, freshly read every call.

## What this verb computes (the deterministic part)

`harness boot` runs pij's readiness proof, in order, with no shell:

0. `just worktree-deps --check` — linked-worktree dependencies resolve before any TS tool runs.
   NOT-READY returns an advisory (`ready: false`, exit 0) with `just worktree-deps` as the fix;
   typecheck/smoke/vitest are not attempted. Rust-only work can continue.
1. `just typecheck` — the whole TypeScript surface (extensions + harness) compiles (60 s deadline).
2. `just smoke` — the bounded smoke proof (~45 s, 90 s deadline). Pass `--full` to run
   `just test` (the whole vitest suite, minutes, 600 s deadline) instead — that is the
   merge gate's job (`just self-check`), not boot's.

Envelope `data` carries `{ ready, mode, stages[], orientation }`. Each `stages[]` entry is
`{ name, cmd, ok, code, elapsed_ms, timed_out }`. `data.ready: true` means all stages passed;
`status: error` means a stage exited non-zero (`boot-<stage>-failed`) or hit its deadline
(`boot-<stage>-timeout`, exit 124) and `data.details.output` holds the last ~30 lines of
that stage's STDOUT and STDERR combined (tsc reports on stdout). It short-circuits: a
failed typecheck does not run the second stage.

## Your role (the inference part)

- `status: ok` with `data.ready: true` → pij is ready; proceed with the-flow / the task. This is a *readiness*
  proof, not the full merge gate — before declaring a task done, still run
  `harness checks` (or `just self-check`).
- `status: error` → read `data.details.output`, fix the named stage, re-run `harness boot`.

## Watch out for

- Boot deliberately excludes `lint` and the Rust gate and defaults to bounded smoke, not the full suite.
  A green boot is *not* a green `self-check` — don't treat it as merge-ready.
- `just` must be on PATH and run from the repo root (boot uses `ctx.cwd`).
- `code: 127` on a stage means the binary (`just`) could not be spawned, not a real failure.
