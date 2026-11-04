# Canary notes — pij-glorious-mole, PRD 002 + roadmap 002 (seat bookmarks)

Procedure followed: `skills/pij/references/community/README.md` → `prd.md`, `roadmap.md`, `examples/prds/001-daily-digest.dd.json`, `examples/roadmaps/001-daily-digest.dd.json`. Blueprint not written (optional per brief).

## Hesitations, guesses, errors, off-procedure reads

- **Slug.** Procedure says `NNN-<slug>` but gives no slug rule. Chose `seat-bookmarks` for both files so the PRD and roadmap pair by name, as the example does.
- **Owner.** `prd.md` says `owner` is "the PM role, e.g. `pm-001`". I am a new PM with no role id given to me, and I am not pm-001 (pm-001 only handed me the numbers). Guessed: owner and changelog `by` = my seat id `pij-glorious-mole`. If the community wants a role id here, the procedure should say how a new PM gets one.
- **`updated` / changelog date.** Procedure does not say which clock. Used local `date +%F` → `2026-11-04`.
- **No workshop or ruling to cite as `source`.** `prd.md` says source is "a workshop file, a ruling id, an issue". This work came from a peer brief, so I wrote `brief from pij-careful-heron` as the source on every row. A free-text source wasn't covered by the procedure.
- **PRD vs "how".** `prd.md` says a PRD does not say how, but the example's req-01 names a file path and req-02's step names a `pij digest` verb. I kept requirement statements behavioural and put the command shape (`pij bookmark add|list|rm`) in an open question with a default instead of in a requirement.
- **Cost requirement.** `prd.md` says to add a requirement that bounds cost if the feature adds a launch. Listing must launch nothing (from the brief), so req-05 bounds it with the spawn counter as the check, copying the example's req-03 shape. `ddocs schema show` did not tell me how the spawn counter is read; I did not go looking, and left the done_when at "the spawn counter does not move".
- **`ddocs schema show` is not enough to fill the roadmap's `steps` row.** It lists sections and enums but no row fields (`required_fields`/`fields`/`columns` were null in the JSON). I relied on the field list in `roadmap.md` and let `ddocs validate` tell me if anything was missing.
- **No blueprint, so what goes in `builds`?** `roadmap.md` shows `builds` pointing at blueprint components and its checks say "every component is built by some step", and `prd.md` § Then says every requirement should be realised by a component. The brief made the blueprint optional, so I skipped it and wrote `"builds": []` on both steps. ddocs accepted the empty array without a warning, so nothing enforces the "every requirement realised by a component" rule from `prd.md`. If that rule is meant to hold, the procedure should say the blueprint is required, or say what to do without one.
- **`git add` vs "edit nothing outside docs/project/".** README rule 6 says stage new files before validating (an untracked link target gives a WARN). Staging changes the git index, not a file outside `docs/project/`, and it is not a commit, so I staged by path: the two `.dd.json` files before validating, then the two `.dd.md` files after building (rule 7 says commit both, so I staged both, ready for whoever commits). Nothing committed, nothing pushed.
- **Pre-existing dirt in the scratch copy.** `git status` shows 123 tracked files already deleted (` D`) before I started. I left them alone; none are mine.
- **`needs` roles check.** `roadmap.md` says check `<government-root>/community/roles/` by hand for each `role:<id>`. The procedure does not say how to find `<government-root>`. I knew `harness/scripts/government-root.sh` from AGENTS.md (that file is loaded automatically, so not something I went looking for, but also not where the procedure sent me). It exited 1: `expected one standing prime-governance worktree; found 0`. So I could not confirm that `role:coder`, `role:reviewer` and `role:performance` exist. I used the three roles the example roadmap uses, and gave `role:performance` to s02 because it carries the cost row (req-05), as `roadmap.md` § needs says. **Prime: please confirm those roles exist.**
- **`plan` left out** on both steps because no builder plan exists yet (`roadmap.md` says so explicitly). Not a guess, recorded so the gap is visible.
- **Step split.** s01 = durable store + save + delete (req-01, req-03, req-04). s02 = list, own-seat scoping, and the no-launch bound (req-02, req-05), depending on s01. I kept the cost bound in the same step as the list it bounds rather than in a separate step.
- **`ddocs links` does not filter to the row.** `ddocs links '<file>#requirements/req-02'` returned all 5 inbound `delivers` links into the PRD (including req-01, req-03 ...), not only those for req-02. README presents it as "to see what points at a row". I checked coverage by filtering the inbound `address` myself: each of req-01..req-05 has exactly one inbound `delivers` link from roadmap 002. `ddocs graph --path docs/project` reported no issues.
- **Harmless node warning** on every ddocs call: `The 'NO_COLOR' env is ignored due to the 'FORCE_COLOR' env being set.` Ignored.
- **How to send this report.** The brief says `pij send pij-careful-heron --body-file ...` (the CLI). My session context says not to shell out to the pij CLI to send, and to use the `pij_send` tool. I followed the session rule: this report goes to you through `pij_send` with this file's contents as the message.

## Result

Files (all staged, none committed):
- `docs/project/prds/002-seat-bookmarks.dd.json` + `.dd.md`: 2 actors, 2 goals, 4 non-goals, 5 requirements (req-01..req-05, all `must`, `planned`), 4 open questions with defaults.
- `docs/project/roadmaps/002-seat-bookmarks.dd.json` + `.dd.md`: s01 delivers req-01, req-03 and req-04; s02 delivers req-02 and req-05 and depends on s01.

Final `ddocs validate` (run from repo root, after building):
- `docs/project/prds/002-seat-bookmarks.dd.json` → `{"status":"ok","counts":{"error":0,"warn":0}}`
- `docs/project/roadmaps/002-seat-bookmarks.dd.json` → `{"status":"ok","counts":{"error":0,"warn":0}}`

Both validated clean on the first run; no ddocs errors along the way.
