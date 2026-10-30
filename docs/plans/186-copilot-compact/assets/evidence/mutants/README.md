# Plan 186 mutants

`run.py <name> <out>` applies one mutation, runs the named tests, and restores the
file. Each receipt here shows the mutation and the tests it turned red. After the
six runs, the restored tree was green: `cargo test -p pij-core --lib control::`
(5 passed), `-p pij-store --test queue_copilot_control` (8 passed),
`-p pij-daemon --test copilot_compact` (3 passed), `--test control_commands`
(8 passed), and `git status` showed no stray edits.

| mutant | what it breaks | red tests |
|---|---|---|
| M1 | Copilot compact still refused | core `copilot_allows_compact_only…`; daemon `control_copilot_compact_queues…`, refusal table; all 3 `copilot_compact` |
| M2 | Copilot `new` allowed | core `copilot_allows_compact_only…`; daemon refusal table |
| M3 | Copilot bodies claimed by the pointer drain | worker `copilot_drain_never_claims_native_bodies…`; all 3 `copilot_compact` |
| M4 | compact typed while the composer has a draft (gate removed; rerun on the real Copilot 1.0.105 draft frame, review N1) | `copilot_compact_is_never_typed_over_a_composer_draft` |
| M5 | busy brake removed (rerun after review S3) | `copilot_compact_waits_for_a_working_seat…`, `copilot_compact_waits_out_an_idle_blip_inside_a_turn` |
| M6 | a waiting compact blocks native parking | `copilot_waiting_compact_never_blocks_native_parking_on_sqlite` |
| M7 | single-read busy brake (review S3) | `copilot_compact_waits_out_an_idle_blip_inside_a_turn` |
| M8 | paneless Copilot seats dropped from the drain list (review S1) | `an_abandoned_copilot_control_claim_never_wedges_native_bodies` |

**Voided first run.** The first run restored each file from a backup whose mtime
predated the mutated build, so cargo reused the mutated object and each later
mutant ran on top of the one before it. The unmutated branch then failed the
draft test 3/3 with a clean `git status`. `run.py` now touches the file before
testing and after restoring, and all six were rerun from a clean build. The
receipts here are from that rerun.
