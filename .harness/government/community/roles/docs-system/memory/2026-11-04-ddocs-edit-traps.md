# ddocs edit traps in pij-fleet

tags: docs-system, ddocs, biome, canary, omp
hook: ddocs from repo root only, git add before validate, biome format after every ddocs write; OMP canaries need a plain clone.

- `ddocs validate` from a subdirectory can't find `.dd/schemas` (E401). Run from the repo root.
- A link to an untracked file WARNs ("address target is not tracked") until `git add -- <file>`.
- `ddocs set`/`add` write JSON that biome rewraps, so `just lint` fails on `docs/project/**` (biome ignores `.dd/` and `docs/plans/`). Run `node_modules/.bin/biome format --write <file>` after every ddocs write. Plan 190's PRD needed a follow-up commit (e26e96e) for this.
- `ddocs links '<file>#requirements/req-NN'` returns every inbound link to the file. Filter: `jq '[.data.inbound[]|select(.address|endswith("/req-NN"))]'`.
- Plan open_questions rows have no id, so ddocs can't address them. Edit with python json (indent=2) to avoid whitespace churn; `jq --tab` churned the file.
- The roadmap step enum is planned/in-progress/complete/abandoned, with no `shipped`.
- OMP refuses to spawn in a linked worktree. For a canary, use `git clone` and `rm -rf .omp .copilot`. `pij close` only tombstones the seat; I removed the canary with `tmux kill-pane` on its own pane.
- Fleet worktrees have no node_modules, so `harness checks --quick` fails on env. Run `~/pi-hacking/pij/node_modules/.bin/{biome,tsx}` against the worktree instead.
