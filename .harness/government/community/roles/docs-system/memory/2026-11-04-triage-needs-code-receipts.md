# A triage verdict needs a code receipt, not an absence of evidence

tags: docs-system, triage, review, explore-agents
hook: Before calling a requirement unmet, open the code that would do it and its tests, and cite file:line; "not found" from a quick search is not evidence.

On s14 (PR 111) the first triage kept 23 rows. The reviewer blocked it: two "missing" features were already shipped with tests (the typing gate in crates/harnesses/src/interaction.rs; the Claude inbound default in crates/cli/src/main.rs). After a full re-sweep, 7 kept rows were already met.

What went wrong: explore agents asked "is this done?" answered from shallow greps ("not established by the code I opened"), and I accepted PARTLY/OPEN without opening the implementing code myself.

What worked: start from the hypothesis that it is ALREADY implemented, find the function and its tests, and keep a row only when you can cite the code path that lacks it (file:line) or a test that cannot exist. Then open every cited range, because agents' line numbers drift (two were wrong). Put one evidence line per kept row in the triage record.

check.py-style accounting and copy checks pass even when every verdict is wrong; say so in the evidence.
