# canary notes — prime pij-premier-hyena, project digest

## Hesitations, guesses, surprises

- Route step 1 says "core roles and security wait under `--suggest` until a need points at them", but the plain `pij directory` view already listed coder and reviewer (as core, "needed by" s01/s02) alongside performance. Consistent in spirit (a need points at them), but the route wording made me expect only performance in the plain view.
- Route links `00-routing.md` § Shared conventions (C18/C22/C24/C26/C28/C30), `../prime/rituals/bootstrap.md`, `docs/how/pij-rs-api.md` and `../community/README.md`. I did not open them; the CLI output was enough so far. Ruling numbers stay opaque to me.
- The plain view's last line was cut at 768 bytes by my tool harness (not by pij); read it via the artifact. Not a pij problem.
- Project docs name `owner: pm-001` (PRD, blueprint, roadmap), but `--suggest` prints `pij directory add pm ...`. The pm template says "One instance per roadmap, named pm-<roadmap number>" and has `instance_key: <roadmap number>`. Two readings: role id `pm-001`, or role `pm` with instance `001` (`role:pm/001`). The route's line "A role counts when its id or its template matches (`pm-001` from `pm`)" tips me to role id `pm-001`, which also matches the docs' owner field. Guess: add `pm-001 --from template:pm`.
- `--suggest` printed the pm add line as `pij directory add pm ...` even though the template's own summary says to name it pm-<roadmap number>, and the docs already say `pm-001`. The suggestion could read the roadmap number and print `pm-001`.
- Template bodies keep `<roadmap number>`, `<project>` and `<government-root>` as literal text. `add --from template:` filled in none of them, so prime's committed packet still reads "for <project>" and "`<government-root>/community/README.md`". I don't know if they get filled in at send time (unverified guess). For pm-001 I filled in `<roadmap number>`→001 by hand in the title, rules and packet, and left `<project>`/`<government-root>` as they were to match the other roles. pm-001's summary still says "named pm-<roadmap number>"; I left it.
- I also set `--instance-key 001` on pm-001 (the template has `instance_key`). With id `pm-001` that may be redundant, or it may mean sends need `role:pm-001/001`. Unverified; the route doesn't say.
- Flag overrides: `--rule`/`--scope-path` on `add --from template:` **replace** the template's lists. They don't append. `add --help` doesn't say which; I confirmed it by reading the committed role.dd.json.
- The coder template's scope path is the placeholder `<the paths named in the step's packet>`, and it would have gone into ROUTES.md as-is. I overrode it with the two blueprint 001 component paths (`crates/daemon/src/digest.rs`, `crates/cli/src/digest.rs`) so routes stay true. Guess: a scope path is meant to be concrete.
- The performance role has no template. The view's suggested command is all `<placeholders>`, so I wrote the rules and packet myself from PRD req-03 and blueprint d-1/k-1, using the security template's packet as the model. I chose `--kind specialist` (like security), though the suggestion said `general`.
- The prime role shows "no holder" in the view even though the header says `prime: pij-premier-hyena`. The route says the prime is designated, never assigned (C24), so I didn't assign. Still, the "no holder" line looks contradictory.
- Every template packet tells the holder to read `<government-root>/community/README.md`. After my adds, `community/` holds only `ROUTES.md` and `roles/`, so no README. The route's step 7 links a `../community/README.md` (in the skill tree), which I didn't read. Nothing told me whether the prime must write the community README.
- Tooling surprise (mine, not pij's route): running `pij directory templates --show pm` from my Python eval kernel, which has no `PIJ_*` env, failed with `E-RS-ARG directory add: unsupported --show`. Same binary (`<scratch>/bin/pij`, a shell wrapper). From bash with `PIJ_*` set, it worked. The error names the wrong verb (`add`), which is odd. I switched to bash for every pij call.
- Read outside the route: none, apart from the repo (README.md, docs/project/*) and `git worktree list`/`git log` on the governance worktree `../digest-gov` to check the commits. I opened the generated `role.dd.md`/`role.dd.json`/`ROUTES.md` there only to read them, never to edit them.
- No holders assigned; no seats spawned. `main` is untouched. Seven commits landed on `prime-governance`, one per add.
- The final `pij_send` to pij-canary-operator came back as `queued ... (cold-check: unknown: no harness session bound)`. I can't tell whether the operator was woken.

## Roles created (7)

| role | from | signal |
|---|---|---|
| prime | template:prime | core (C22); the designated prime (pij-premier-hyena) needed its role document. Not assigned (designated). |
| pm-001 | template:pm, title/rules/packet filled for roadmap 001, scope `docs/project/**` | core; the PRD, blueprint and roadmap 001 already name `pm-001` as owner; s01/s02 need packets, assignments, a plan number. |
| coder | template:coder, scope paths = blueprint component paths | core; needed by roadmap 001 s01 and s02 (`needs: role:coder`). |
| reviewer | template:reviewer | core; needed by s01 and s02; a judgment must be independent of the author. |
| security | template:security | recommended for every community; a judgment must be independent of the author; pm-001's rules ask it for a verdict on every PR. |
| pa | template:pa | core; worth remembering (C30): the long-lived holders (pm-001, security, performance) need memory before compacts/closes. |
| performance | written fresh (no template) | roadmap 001 s01 `needs: role:performance`; PRD req-03 "no cost when idle" and blueprint cost k-1 need an independent cost verdict. |
