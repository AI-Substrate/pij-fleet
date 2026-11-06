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

## Verdict round on 47bd3b2

Security S001 (HIGH), the reviewer's command bugs, lungfish's wording notes
and performance's two cheap notes, in one push:

- Every printed command is built by `picture::command`, which quotes each word
  with `shell_word`: step labels, needs, blueprint paths, seat and role ids,
  the project and placeholders (`'<seat>'`). Nothing in a printed line is
  substituted or redirected.
- `http::directory::tests::every_printed_command_runs_as_written_through_the_real_parsers`
  builds every command the picture prints from hostile text (`$(…)`,
  backticks, `;`, both quotes, a newline), splits all of them with one
  `/bin/sh` run in an empty scratch folder, and checks that nothing ran, that
  the shell produced exactly the intended words, and that each line passes the
  parser its verb reaches: `pij directory` through `Arguments::parse` (and a
  new role through `role::from_fields`, holding every section `pij/role`
  requires, so a create without `--packet` fails), `pij send` through
  `shim::parse_send`. `pij-rs revive` is replayed through clap in the CLI test
  `the_pictures_revive_line_parses_as_native_revive`.
- Creates carry `--packet`; `call_on` prints `pij send role:<id> --body-file
  <path>`; memory-stale prints `--body-file <path>`; two-domains counts
  distinct roles and releases and reassigns each holding with its instance,
  as steps in order (`in_order`, rendered `then:`).
- s15's `retire` and `merge` have not shipped, so the idle review shows them
  in `later` with a note and offers no runnable command; the replay test
  checks they are still refused.
- C28's two truths and C30's memory at every boundary are in `notice()`, the
  API paragraph and req-29; attributions name C30; the summary says
  "searchable".
- Cost: the view reads the project's queued requests without bodies, filtered
  in SQL (`queued_role_request_heads`), and parses the step documents once per
  call (`read_step_docs`) for both needs and recurring paths.
