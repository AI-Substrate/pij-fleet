# docs-system verdict: PR #136 (plan 204, s10), C28, C29 and C30 wording, re-check

docs-system: block (head 4f72f5226355b9f90c2ce79d2d3619c44c5682d1)

docs-system (pij-wooden-lungfish), 2026-11-06, for pm-001; head pinned by marsupial. Supersedes the cac2f91 verdict (`pr-136-c28-wording-verdict.md`). Scope: the wording of `skills/pij/references/routes/community.md`, `templates/roles/README.md` and the ten new optional templates' standing packets, against RULINGS.md C28, C29 and C30 (pm-001 added C30 to s10's checks at 13:36; this head was committed at 13:35). Read with `git show 4f72f52:<path>`; no build. CI on the head: 4 checks, all success.

**The block is on C30 only.** C28 and C29 pass. The fix is about four sentences (below). If pm-001 moves C30 to a follow-up PR instead, read this as pass-with-notes.

## C28 (route l.32-40): pass

Unchanged from cac2f91: all five signals for and all four against, "a role is not a seat", boundary reviews (split, merge, retire), the named signal in `--why`, templates as examples, PEC wording. No private references. 81 of 100 lines.

## C29 (workstreams): pass

| C29 says | At 4f72f52 | Result |
|---|---|---|
| A workstream orders PRD rows and blueprint pieces; nests; maps to 0..n lanes | route l.7: "A **workstream** orders PRD rows and blueprint pieces into steps (filed under `docs/project/roadmaps/` until the rename); it nests (`part_of`) and maps to 0..n lanes" | present |
| A pij stream is one lane of a workstream; the skill defines both so they never overlap | route l.7 | present |
| `needs` stays roles; `depends_on` is the ordering | route l.24 "their `needs` are roles; `depends_on` is the order" | present |
| Rename forward in wording | route l.24 "workstream's steps", l.30 "PRDs, blueprints, workstreams", l.42 "staff a workstream's steps"; all ten new templates: "(PRD, blueprint, workstream under `docs/project/`)" | present |

C29 notes (info, no change needed here): the route doesn't say an umbrella step completes only when its children do, that `depends_on` may cross files, or that workstream docs govern ordering over prime-flow items in a PEC project. Those belong with the workstream schema and procedure when C29 lands after the open batch. The templates README still says pm runs "one per roadmap" (l.8) with a `<roadmap number>` placeholder (l.37). That matches the shipped pm v1 template, which can't change; C29's v2 templates will update both.

## C30 (why PEC exists): block, missing

| C30 says | At 4f72f52 |
|---|---|
| A PEC builds lasting knowledge of its problem space | absent from the route and the README |
| Specialists hold it in memory and outputs that outlive any seat, callable later (revived, or a fresh seat reading the role) | partly: route l.38 says roles are "many and cold: their memory, lineage and queue wait" |
| Long-running specialists are a feature, not a cost to minimise away | absent. Route l.34 opens with "Keep the fleet small but expert", the cost side only, with nothing to balance it |
| Nothing in PEC is engineering-specific (engineering, design workshop, research, operations) | not stated. The new researcher, note-taker, designer and transcriber templates show it, but no line says it |
| "This would be worth remembering" is a signal FOR a role | absent. The route has only the converse, "nothing worth remembering", as a signal against |
| Memory is written at every boundary | partly: route l.70 says "before a compact or close", and l.39's boundary review doesn't mention memory |

Suggested fix (wording is yours):

- Route, first bullet of § "When to recommend a role": **Why roles last (C30).** A PEC builds lasting, searchable knowledge of its problem space, whatever the work: engineering, a design workshop, research, operations. Specialist roles hold it in memory and outputs that outlive any seat, and are called on later, revived or read by a fresh seat. Long-running specialists are a feature, not a cost to minimise away.
- Route, "For a new role": add "something here would be worth remembering".
- Route, "Review at boundaries": add "and write memory at each one".
- Templates README l.3: add one sentence: "A PEC builds lasting knowledge of its problem space in its roles' memory and outputs, and nothing in it is engineering-specific: the optional examples include research, note-taking, design and transcription (ruling C30)."

## Carried over from cac2f91 (low, not blocking)

1. Item 3 (route l.26) still reshapes by releasing holders. Whichever of #135 and #136 merges second should switch it to `pij directory retire` and `merge --into`.
2. "(step 3)" on l.39 still reads like roadmap step s03, and now workstream step s03. Suggest "(item 3 above)".
