# Plan 203 evidence: the prime's operating picture and prompts

## Cost (bp-0006, AC6)

Private daemon, scratch repos, private tmux server and port. Script:
[private-run.sh](private-run.sh); transcript: [private-run.txt](private-run.txt)
(home path and hostname scrubbed). Readings are `pij-rs --json ping`
`data.spawns.by_op`, same daemon pid throughout. Built niced from this branch
(head 8440d5d's sources). The project had two roles, one seat holding both, a
memory note, an output, a roadmap with three steps building one blueprint path
and a step need, so the view computed every column, prompt (core, security,
needed, recurring) and review (two domains).

| Step | total | Note |
|---|---|---|
| at rest, 60 s apart | 70 -> 70 | flat: nothing on a timer |
| ten `pij directory --json` views | 70 -> 70 | the view launches nothing |
| one human `pij directory` | 70 -> 70 | the same |

Reads per call (from the code, bp-0006's note): store queries for the
holdings, the project, the queued requests and each role's newest output (one
window query), plus one registry list; in-process reads of each role's document
and memory folder, the step documents (workstreams, else roadmaps) and the blueprints they build. Nothing
per role or per seat launches, and nothing runs on a timer or poll. No new
launch site, so `crates/testkit/tests/spawn_sites.rs` needs no change (green in
CI).

Live daemon before deploy (2026-11-06 about 06:32Z, read-only, no bounce):
`pij-rs --json ping` spawns 2813 then 2855 60 s apart (42, 0.7/s).
`just spawn-rate --seconds 20`: 157.8 launches/s sampled machine-wide (several
lanes building), `pij-rs daemon` children 0.8/s. The after-deploy reading
belongs to the deploy.

## Tests (CI on #141)

- `community::picture` (9): waiting, actions (no live holder, two live
  holders, memory stale), roadmap needs, needed prompts, recurring unowned
  paths, two domains, idle (a cold role with memory is kept), and the C28
  notice built from the same signals.
- `directory_http::the_view_shows_the_picture_and_prompts_the_prime`: AC1 to
  AC4 and AC8 over HTTP in a scratch project, each recommendation naming its
  signal; adding a needed role clears its prompt.
- `pij-rs` `the_directory_view_renders_the_picture_in_local_time` (AC5: local
  time with zone, one line per holder, signals, the boundary review).
- `store_integration::outputs_answer_for_their_exact_head_and_list_by_role`
  (`latest_outputs`).
