# canary notes (prime of `digest`)

- Read outside the route (listing only): ran `ls -la <scratch>/` to see whether canary-notes.md existed. Saw brief.md, env.sh, operator-register.json, project-create.txt, daemon-home/, state/, digest-gov/. Did not open any of them.
- Did not load the `pij` skill (SKILL.md) since the brief forbids other pij skill files.
- Route links four other files: 00-routing.md (§ C conventions), prime/rituals/bootstrap.md (governance branch), docs/how/pij-rs-api.md (community directory API), community/README.md (doc procedures). I read them only if I need them.
- Workstream 001 steps: s01 needs role:coder, role:reviewer, role:performance; s02 needs role:coder, role:reviewer. Docs' `owner` is pm-001.
- `pij directory` view: 0 roles; prime is pij-informal-scallop (I assume that is me; the route never says how to confirm the prime's own seat id). Prompts: coder, reviewer (core, needed by s01+s02), performance (needed by s01). `--suggest` adds prime, pm, pa, security.
- Mismatch: `--suggest` prints `pij directory add pm --from template:pm`, but route step 2 says `add pm-001 --from template:pm --instance-key 001` (the id the docs' `owner` names). I follow the route.
- No performance template exists. The view's suggested line uses `--kind general` and `<title>`/`<rule>`/`<packet>` placeholders. I wrote it from scratch as `--kind specialist` (independent verdict, like security). Guess.
- Read the linked bootstrap.md section (lines 25-49) only, to resolve `<government-root>`. To find that section I ran `ls -R` on ref/skills/pij/references (file names only, outside the route's links) and grepped bootstrap.md headings.
- `<government-root>` resolves (via `git worktree list --porcelain`, prime-governance record) to <scratch>/digest-gov/.harness/government. It holds only a one-line README.md; no remote configured.
- Guess: in standing packets I replaced `<government-root>` with "`.harness/government/community/README.md` on the `prime-governance` branch" rather than the absolute /private/tmp path, because the template's own Never-list forbids committing machine-local paths. The route does not say which form to use.
- Packet texts contain backticks and quotes, so I keep them in files under the session folder and pass them with `--packet "$(cat file)"` from a script, to avoid shell-guard blocks. Route does not cover passing long packets (only `--packet <text>` or `--brief <repo-relative path>`).
- First add (prime) worked with `--from template:prime --scope-path ... --packet "$(cat file)"`; the writer committed eb3140b in the gov worktree and also seeded `.dd/schemas/pij/*` and regenerated community/ROUTES.md. `routes --write` was never needed.
- Surprise: every add sent an FYI to the prime (pij-informal-scallop), 7 in all (msg ids b0c21bb319e6bba5-...0 to ...6). The route doesn't mention this.
- Surprise: the writer's commits are authored "Jordan Knight <<auto-detected email>>" (machine git identity, a local hostname), while earlier gov commits used other identities. Could leak a machine-local detail if the branch were ever pushed public.
- README: the route says to write `<government-root>/community/README.md` once by hand, but not whether to commit it. I committed it myself by named path (631f538, with a Copilot co-author trailer) right after the first add, guessing an uncommitted file there could trip E-RS-GOV-FOREIGN-EDIT on later writes. No refusal followed.
- Guess: prime got scope path `.harness/government/**` (it "owns the community directory and the rulings"). This overlaps the writer-managed files; ROUTES reported no overlaps.
- Guess: coder scope paths are the two blueprint component files only (crates/daemon/src/digest.rs, crates/cli/src/digest.rs). Tests and plan folders are unknown until code exists.
- Guess: pm-001 also owns the three 001-daily-digest docs (roadmap, PRD, blueprint), since their `owner` fields say pm-001. I rewrote the pm summary because the template's summary still read "named pm-<roadmap number>".
- Guess: performance got `--harness omp` to match the templates' default seat. No template to compare against otherwise.
- After all adds the view lists `prime (general, one): no holder`. It is correct (the prime is designated, not assigned, and the header names pij-informal-scallop), but at a glance it reads as an unheld role.
- Not read: 00-routing.md, docs/how/pij-rs-api.md, community/README.md (doc procedures). Nothing refused, so I did not need them.
- Verified: view shows 7 roles and no actions/prompts; `--suggest` prints nothing; `routes` lists every scope; no `<project>`/`<government-root>`/`<roadmap number>` left in any role.dd.json; gov worktree clean. Holders all empty; nothing assigned, nothing spawned.

## Roles created (all in the prime-governance worktree, .harness/government/community/roles/)

1. prime (template prime): the prime pij-informal-scallop is designated; this adds its document. Signal: a core role a PEC starts from (C22).
2. pm-001 (template pm, instance 001): runs roadmap 001; the PRD, blueprint and roadmap all name pm-001 as owner. Signal: core role; a path needs one owner.
3. pa (template pa): memory before compacts and closes, so specialist knowledge outlives seats. Signal: a core role a PEC starts from (C22).
4. coder (template coder): roadmap 001 s01 and s02 both list role:coder in `needs`. Signal: a workstream step names this role.
5. reviewer (template reviewer): s01 and s02 both list role:reviewer. Signal: a judgment must be independent of the author.
6. security (template security, plus one digest rule about what the job copies into digests/): recommended for every community and asked for in the brief. Signal: a judgment must be independent of the author.
7. performance (no template, written from scratch, specialist): s01 lists role:performance; req-03 (no cost when idle) and blueprint cost k-1 (once a day, 0 launches) need an independent cost check. Signal: a workstream step names this role in its needs.

- Post-script: the done line to pij-canary-operator returned outcome "queued" (reason "extension-stream", cold_check "unknown: no harness session bound"), not delivered. It also confirmed my seat is pij-informal-scallop, so the 7 FYIs above went to me.
