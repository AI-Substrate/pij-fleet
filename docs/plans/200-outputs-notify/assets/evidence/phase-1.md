# Plan 200 evidence: role outputs and notification

## Cost (bp-0005, AC5)

Private daemon, scratch repos, private tmux server and port. Script:
[private-run.sh](private-run.sh); transcript: [private-run.txt](private-run.txt)
(home path and hostname scrubbed). Readings are `pij-rs --json ping`
`data.spawns.by_op`, same daemon pid throughout. Built niced from this branch
with plan 205 (s05, s12) merged.

| Step | git.community | ddocs.community | other | Note |
|---|---|---|---|---|
| at rest, 60 s apart | 0 | 0 | 0 | flat: nothing on a timer |
| `send --to role:reviewer` (reply wake) | 0 | 0 | 0 | s05's send; recorded as a role request |
| `output verdict --answers <request>` | +7 | +2 | 0 | one writer commit (the `.dd.json` and its `.dd.md`); the request is marked answered; the asker is told in `wake` mode, one delivery (`queued`, extension stream) |
| unrequested `output note` | +7 | +2 | 0 | one writer commit; one FYI held for the writer's parent |
| two `output note` at once | +14 | +4 | 0 | two commits, two files (`…-pr-201.dd.json`, `…-pr-201-2.dd.json`), each with one FYI |
| `outputs --subject … --head …`, `outputs --role …` | 0 | 0 | 0 | store reads |

So an output costs one writer commit, 9 launches (git 7, ddocs 2), as the plan
said, and its notification is one delivery that launches nothing. No loop, poll
or timer was added, and it doesn't scale with seats: the cost is per output
written. No new launch site (`output.rs` and `community/output.rs` launch
nothing; the writer is the existing one), so `crates/testkit/tests/spawn_sites.rs`
needs no change and runs in CI.

Live daemon before deploy (2026-11-06 about 03:09Z, read-only, no bounce):
`pij-rs --json ping` spawns 13790 then 13859 60 s apart (69, 1.15/s on the
working fleet). `just spawn-rate --seconds 20`: 96.1 launches/s sampled
machine-wide, `pij-rs daemon` children 1.2/s. The after-deploy reading belongs
to the deploy.

## Tests

`crates/daemon/tests/output_http.rs` and the store's outputs tests (AC1-AC4),
green in CI on #137.

## Verdict round on 4ab6460

Main merged at 2c6fa1e (#129, #133 and the rest), and the PR retargeted to main.

- F001: `community_outputs` is keyed by `(project, path)` (migration 0036
  amended, unmerged) and `record_output` conflicts on both;
  `store_integration::outputs_answer_for_their_exact_head_and_list_by_role`
  indexes the same relative path in two projects and reads both back.
- F002: the notice's `msg_id` is `output-<project>-<commit>`, never the clock;
  `output_http::two_outputs_at_once_both_notify_under_their_own_commits`
  writes two at once and finds both FYIs held, each under its own commit.
- N001: a failed lookup of the seat above, or a refused forced-wake audit,
  reaches `notified.outcome` with its cause.
- S001 (pm-001's ruling): `--answers` only from the request's recorded holder,
  a live holder of its role or its project's prime; anyone else gets
  `E-RS-OUTPUT-NOT-HOLDER` before any write, stamp or send.
  `output_http::only_the_holder_a_live_holder_or_the_prime_may_answer_a_request`
  covers the refusal (no commit, index, stamp or notice) and the live-holder
  and prime cases; the reply-mode test covers the recorded holder.
