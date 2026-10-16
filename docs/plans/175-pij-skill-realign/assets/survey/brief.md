# pij skill survey: shared brief

Goal: check that the pij skill (`~/pi-hacking/pij/skills/pij/`, main @ bb9f0b1) matches current-day pij. The Rust daemon/CLI (pij-rs) is the live generation; the TypeScript daemon is legacy.

## Ground truth (in priority order)
1. Live CLI help: `pij --help`, `pij <verb> --help`, `pij-rs --help`, `pij-rs <verb> --help` (read-only, safe).
2. Code: `crates/` (cli, daemon, core, store, harnesses), `harness/scripts/`. Use `grep -rn`, or `rg --hidden` (rg skips .omp/.copilot otherwise).
3. Docs: `docs/how/*.md` (esp. `pij-rs-api.md` Unsupported status), recent plans `docs/plans/1[5-7]*/`, `git log --oneline -80`.
A claim in the skill is only "OK" if you checked it against 1 or 2. Docs alone are weaker evidence; say so.

## For each file in your partition, read it IN FULL, then check every concrete claim:
- verbs, flags, subcommands, error codes (E-RS-*), env vars, paths, file names, recipes (`just …`)
- "unported / E-RS-UNPORTED" claims (is it still unported, or shipped since?)
- behaviour claims (delivery modes, watchdog, compaction, revive, close, adopt, cold-wake guard, fyi, PA, report cadence)
- links to other files/anchors (do they resolve?)
- references to retired things (legacy TS daemon, flow-pair stubs, `pij tail`, the-flow files, old plan numbers)
Also note shipped features the file SHOULD mention but doesn't (only where it's that file's job).

## Hard rules
- READ-ONLY. Never edit the repo, never run `pij send/spawn/revive/close/adopt/compact-self/watchdog on|off/report` or anything that mutates. `--help` and `pij state`/`pij list`/`pij whoami` reads are fine. Never touch tmux panes, ~/.pij-rs, the daemon, node_modules.
- Never print secrets.

## Output: write `docs/plans/175-pij-skill-realign/assets/survey/<partition>.md`
Per file: a table `| line | claim (short) | reality + evidence (cmd or file:line) | verdict |`.
Verdicts: WRONG (contradicts code), STALE (was true, superseded), UNVERIFIABLE (couldn't check; say why), MISSING (gap), BLOAT (true but token-wasteful/duplicated elsewhere). Omit lines that check out OK; end each file with "N claims checked OK".
Top of the report: a 5-10 line summary ranked by severity (WRONG first).
Reply to me with just the report path and the summary lines.
