# `harness checks` — agent briefing

> Served verbatim by `harness instructions checks`, freshly read every call.

## What this verb computes (the deterministic part)

`harness checks` runs pij's full deterministic gate — the engineering-harness
**signal inventory** made runnable. It mirrors `just self-check` but runs each
sensor as its own stage and runs **all** of them (it does not stop at the first
failure), so one invocation surfaces every problem:

1. `local-paths` — `just local-path-check` (operational source/config contains
   no user-specific absolute home paths)
2. `typecheck` — `just typecheck` (the TS surface compiles)
3. `lockfile` — `just lockfile-allowlist` (every lockfile source is an allowed host)
4. `lint` — `just lint` (Biome errors + warnings clean)
5. `test` — `just test` (vitest suite passes)
6. `copilot-native` — `just copilot-native-test` (see below)
7. `rust` — `just rust-check` (Cargo.lock current, fmt, clippy `-D warnings`,
   workspace tests) — **heavy**
8. `smoke` — `just smoke` (tmux-driven driver scenarios) — **heavy**
9. `pij-commit-trailers` — `just pij-commit-trailers --json` (advisory attribution
   scan of harness-authored/co-authored commits after merge-base with `main`;
   old history is excluded, missing identity is reported rather than guessed).

`copilot-native` runs `just copilot-native-test`: the native extension's
node:test contracts are outside the Vitest include and must not be invisible.
This deterministic sensor does not substitute for `just copilot-native-smoke`
with actual manual/spawned Copilot clients; local-provider and real-provider
witnesses are separately graded.

Envelope `data` carries `{ ok, ran[], skipped[], results[] }`; each `results[]`
entry is `{ name, status: pass|fail|warn|skipped, code, proves }`. Advisory
results retain `output` with missing trailers and exact derivable lines to add;
warnings and unavailable attribution never fail the gate. On failure,
`data.failures[]` holds the last ~25 lines of each failing sensor's output.

`--quick` skips heavy sensors (rust, smoke) for a fast static+unit gate.

## Your role (the inference part)

- `status: ok` (no `--quick`) → **ship/done-ready**: every sensor passed.
- `status: ok` with `--quick` → fast gate green, but run the full `harness checks`
  (incl. rust and smoke) before actually shipping / declaring done.
- `status: error` → read `data.failures[]`, fix every failing sensor, re-run.
  Do not declare the task done while this is red.

## Watch out for

- **`smoke` needs tmux** and is slow; it fails (not skips) if tmux is unavailable.
  Use `--quick` when you only need the static+unit signal mid-change.
- **Scope = tsconfig-included paths only.** typecheck/lint cover `.omp/extensions/**`,
  `harness/**`, `skills/**` (tsconfig `include`) — a broken file under `scratch/`
  (excluded) will NOT trip the gate. That's correct, but don't probe `scratch/`
  to test the failure path; use an in-scope file or you'll get a confusing false green.
- **Output**: the envelope is JSON when piped / `HARNESS_JSON=1`, and a TTY renders
  it human-readably. `--json` / `--no-json` are **global harness flags** (kernel-level,
  so not listed in `harness checks --help`); when output is already piped they are no-ops.
- This is the canonical **done/ship gate** — distinct from `harness boot`
  (a fast readiness proof = typecheck+test only). Boot green ≠ checks green.
- `code: 127` on a sensor means the binary (`just`) could not be spawned.
- Adding a new harness sensor/back-pressure check = add one entry to `SENSORS`
  in `.harness/extensions/checks/extension.ts` (keep it in sync with `just self-check`).
