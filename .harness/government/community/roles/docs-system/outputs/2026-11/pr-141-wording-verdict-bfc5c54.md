# docs-system verdict: PR #141 (plan 203, s09), wording delta

docs-system: pass (head bfc5c54fb41e9a922e30b0863c2829ee13fd4eb2)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001. Supersedes the 47bd3b2 verdict (`pr-141-wording-verdict.md`). Scope: the wording delta `git diff 47bd3b2 bfc5c54`, read in the fresh review worktree without moving it; no build. CI on the head: green.

| 47bd3b2 note | At bfc5c54 | Result |
|---|---|---|
| 1. C28's two truths | `notice()`: "Recommend a role from need, weighing two truths: small contexts are cheaper and sharper, and an expert in its own seat beats a generalist carrying the topic. Keep the fleet small but expert." Also in req-29's statement and the API paragraph | fixed |
| 2. C30's memory at every boundary | `notice()`: "..., and have each holder write memory at the boundary"; req-29: "and each holder writes memory (ruling C30)"; API: "the boundary review, at which each holder also writes memory (C30)". Matches #136's route | fixed |
| 3. `call_on`'s send form | prints `pij send role:<id> --body-file '<path to what you need from it>' --project <slug>`, the form the shim parses and the docs teach | fixed |
| 4. Attributions and "searchable" | `notice()`'s doc comment cites C26, C28 and C30; the API heading reads "Growing the community (rulings C28, C30)"; the PRD summary now says "lasting, searchable knowledge", matching its changelog row | fixed |

Also read: the API doc's new `later` paragraph (s15's `retire` and `merge` shown with a note and no runnable command until #135 lands) and `in_order` (release then assign) are clear and accurate for this base. PRD 001 validates `ok 0/0`. The delta's added lines have no machine paths, private owner/name or hostnames.

No open wording notes.
