# Plan 194 phase 2 evidence: templates, drift and prompts

Phase `ph-94b2`, written by pij-following-marsupial (coder), 2026-11-06. Script [`private-run.sh`](private-run.sh), transcript [`private-run.txt`](private-run.txt). Temp, home and worktree paths are shortened to `<tmp>`, `~` and `<worktree>`.

The script runs a private daemon: temp `HOME` and state, a private `-L` tmux server, port 17904, a scratch repo with a standing `prime-governance` worktree, and one scratch seat bound to the private pane. It never touches the machine daemon, `~/.pij-rs` or the live governance branch. The binary is a debug `pij-rs` from this branch (code at 43c932b), with the real ddocs and the embedded `pij/*` schemas.

## Cost (bp-0006, AC7)

All readings come from one daemon process (pid 26469). The six launches present from the first reading are the seat's pane tap and process-tree probes at registration, unchanged throughout.

| Window | Launches | By operation |
|---|---|---|
| At rest, 30 s | 0 | none |
| One `pij directory templates` | 0 | none |
| One `pij directory` (the view, with six prompts) | 0 | none |
| One `pij directory add security --from template:security` | 9 | git.community 7, ddocs.community 2 (the writer's usual add) |
| One `pij directory template-diff security` | 0 | none |
| One `pij directory` after the add | 0 | none |

Steady-state cost: none. Templates are embedded bytes; listing, diffing and the prompts read in-process. No new launch site, loop or timer; nothing scales with seats or ticks.

## Behaviour seen on the private daemon

- The empty directory's view printed the PEC guidance and six prompts (prime, pm, pa, coder, reviewer, security), each a runnable `pij directory add <id> --from template:<id> --why "…"` line. It wrote nothing.
- The security prompt's line, run as printed, committed `community: add role security` with `meta.template: security` and `meta.template_version: "1"`. The next view listed security and no longer prompted for it.
- `template-diff security` reported v1 as the latest (only v1 ships today). The behind case, a v1 role against a v2 fixture, is proven in `crates/daemon/tests/templates_http/mod.rs` (`drift_shows_the_upstream_change_and_never_applies_it`).

## Tests (bp-0002 to bp-0004)

- `cargo test -p pij-daemon --test directory_http templates`: 5 passed. `add_from_each_template_records_it_and_unknown_ones_refuse_by_name`, `from_file_is_the_brief_spelling`, `drift_shows_the_upstream_change_and_never_applies_it`, `prompts_list_missing_core_roles_and_security_and_create_nothing`, and `real_ddocs_validates_a_role_from_every_shipped_template` (ran here with the real ddocs; it names its skip in CI, where ddocs is absent under C20).
- `cargo test -p pij-daemon --lib -- community:: http::directory`: 50 passed, including `every_template_file_is_embedded_byte_for_byte` (AC3), the catalog, diff and prompt units, and `flags_override_a_template_document_field_by_field`.
- `cargo test -p pij-core --lib -- directory` and `cargo test -p pij-cli --test skill_verb_coverage`: passed.

## Gates

- `cargo fmt --all --check`, `cargo clippy --locked -p pij-daemon -p pij-core -p pij-cli --all-targets -- -D warnings`: clean.
- `ddocs-check`, `local-path-check` and `lockfile-allowlist`, run with the main checkout's `tsx` (this worktree has no `node_modules`, C25): clean. Main-checkout biome on the changed files: clean.
- TS proof (typecheck, vitest, the full `harness checks`) is CI on the PR head. No TS changed: the shim forwards `directory` argv unchanged.
- Scoped Rust gate after merging main (0eebb69): `cargo test --locked --no-fail-fast -p pij-core -p pij-cli -p pij-daemon` ran 67 test binaries, 1270 passed, 5 failed at load average about 40: `http::native_tests` (three prebind/takeover cases), `http::tests::a_live_idle_claude_frame_under_a_long_stale_working_is_refused` and the flake below. None touch the directory code, and all five pass when re-run alone (`ok. 4 passed`, `ok. 1 passed`).
- Known flake, not from this change: `pr122_review::review_reassign_within_one_second_keeps_both_holdings` fails its own same-second precondition when the machine is busy (1 of 2 runs alone); it passed in the full-file rerun.

## Review round 1 (pij-subsequent-reindeer, block at 04144d1)

- F001 (P2): a view selected with `--project remote` printed add prompts and a drift hint without `--project`, so replaying one from another project's repo wrote the role there. Now every printed command ends with `--project <slug>`, quoted with `pij_core::directory::shell_word`, always, so the view's output is the same whichever way the project was chosen. The private-daemon transcript above predates the repair; the cost is unchanged (no new launch).
- Regression: `templates_http::a_prompt_replayed_from_another_repo_writes_to_the_viewed_project` shell-splits the printed security prompt, replays it from the other project's cwd and checks `data.project`, the remote file and no local file. `the_drift_hint_names_the_viewed_project` (pij-rs bin) checks the hint, with a project name that needs quoting.
- After the repair: `directory_http templates` 6 passed; `community::templates` 6 passed; `pij-core directory` 2 passed; the CLI hint test passed.
