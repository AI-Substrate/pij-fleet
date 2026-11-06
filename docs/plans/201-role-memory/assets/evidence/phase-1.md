# Plan 201 evidence: role memory and the memory turn

## Cost (bp-0004, AC4)

Private daemon, scratch repos, private tmux server and port, bound 5 s
(`PIJ_RS_MEMORY_TURN_SECS=5`). Script: [private-run.sh](private-run.sh);
transcript: [private-run.txt](private-run.txt). Readings are `pij-rs --json ping`
`data.spawns.by_op`, same daemon pid throughout.

| Step | git.community | ddocs.community | other | Note |
|---|---|---|---|---|
| at rest, 60 s apart | 0 | 0 | 0 | flat: nothing on a timer |
| two `memory add` at once | +14 | +4 | 0 | 7 git and 2 ddocs per note; two commits of 3 files each, both INDEX lines kept |
| `memory <role>` | 0 | 0 | 0 | in-process read |
| `memory <role> --note` | 0 | 0 | 0 | in-process read |
| `memory delete` (refused), duplicate add (refused) | +3 | 0 | 0 | the duplicate resolves the governance root, then refuses `E-RS-MEMORY-EXISTS` |
| close of a seat holding no role | 0 | 0 | +1 `tmux.pipe-pane detach` | the existing tap teardown; one holdings query |
| close of a holder (turn hit the bound) | +7 | +2 | +1 detach, +2 clear-options | the existing close-end commit and tap teardown; the memory turn itself launched nothing |

The memory turn is one delivery, one live subscription, one `memory.turn`
spine event and one deadline per compact or close of a holder. No poll, no
timer, no launch. `crates/testkit/tests/spawn_sites.rs` passes: no new launch
site.

Live daemon before deploy (2026-11-06, pid 93273, read-only, no bounce):
`pij-rs --json ping` spawns 1478 at 00:42:23Z and 1548 at 00:43:23Z (70 in 60 s,
1.17/s on the working fleet). `just spawn-rate --seconds 20`: 108.8 launches/s
sampled machine-wide, `pij-rs daemon` children 1.7/s. The after-deploy reading
belongs to the deploy.

## Tests

- `community::memory` (6): slug, note shape, INDEX regeneration keeping
  hand-written lines (links, bare names, dotted rows), placeholder removal,
  refusals, reads by name, stem and dated slug.
- `community::writer::tests::two_memory_adds_at_once_land_as_two_commits_keeping_every_index_line`
  and `real_ddocs_accepts_a_memory_note` (real ddocs).
- `directory_http::memory_add_writes_one_note_per_commit_and_reads_back`.
- `http::memory_turn` (4) and `memory_turn_http` (5): a holder's close waits for
  the turn, a non-holder is direct, the bound proceeds and says so, a parent's
  compact is admitted only after the turn, a self compact or close is held and
  runs when the turn ends.
