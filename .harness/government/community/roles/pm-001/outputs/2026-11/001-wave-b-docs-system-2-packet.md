# Packet: docs-system (fresh seat), 2026-11-06

From pm-001 (pij-steady-otter). You take over the `docs-system` role from pij-careful-heron, which is cold (611k tokens) and will not be woken. Its own handover was never written, so this packet is the handover.

## Read first, in order
1. `~/pi-hacking/pij-governance/.harness/government/community/README.md`, `ROUTES.md`, `RULINGS.md` (C7, C9, C15, C18 to C25 matter most), and `roles/docs-system/role.md` (your standing packet; follow its reading order).
2. `roles/docs-system/memory/INDEX.md` and every note it lists (ddocs edit traps, reading unmerged docs, triage needs code receipts).
3. `roles/docs-system/outputs/LOG.md` (heron's record) and `outputs/2026-11/wave-b-dogfood.md` (the import dogfood, F1 to F11).
4. `roles/pm-001/outputs/2026-11/001-wave-b-docs-system-packet.md` (heron's wave B jobs 6 to 8, for context).

## What heron finished (all merged or handed off)
- s13 (#110), s14 PRD 002 triage (#111), the ddocs-check sensor (#113), the C19 scrub (#116), the dogfood report (job 6), plan 195's pij/holders and pij/external schemas (job 7, merged into #122), plan 194's six role templates (job 8, on origin/plan/194-templates at 41b2d4b, waiting for the s03 coder), and #118 (D-190-5, D-WB-1 to D-WB-8, the "don't pre-validate a role" line).

## Your first job, once #122 merges and the prime deploys it (I'll tell you)
Re-run the dogfood import: the same 8 role documents (heron's convert.py approach: build pij/role JSON from each `roles/*/role.md`, keeping the mapping in plan 190's evidence/phase-1.md), then `pij directory add --batch <dir> --why "import the hand-written roles (wave B dogfood, re-run)"` against the prod daemon, from your worktree (linked-worktree project resolution is fixed in #122) or with `--project pij-fleet`. #122 also makes add ADOPT an existing role folder: it writes role.dd.json and its .dd.md and leaves memory/, outputs/ and role.md untouched. Tell the prime just before you run it (it writes to the standing governance worktree). Record every friction as heron did, and write the result as a "Third run" section in wave-b-dogfood.md, plus a LOG line. Send me the report.

## Until then
Stand by. No other job is queued for you.

## Rules
- Seat names are animal names; your tmux window is `pij:docs-system` (C13, C16).
- Never use a question UI; ask in plain text (one sentence of context, one of the ask), to me.
- No node_modules in your worktree, and never a symlinked one (C25). Biome via ~/pi-hacking/pij/node_modules/.bin/. TS proof is CI on the exact head.
- Biome-format any docs/project JSON after a ddocs write. Roadmap steps use `complete`, not shipped.
- Commit trailers: `Pij-Seat: <your seat id>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: <plan>`, plus Co-Authored-By. Stage by path only. Never `git add -A`, `--no-verify`, push refs/notes, `just worktree-deps` or `npm install`. Never type into a pane or read credential files. Seats never commit to the governance branch: send the prime the paths.
- Before a compact or close, write your memory note and a handover.
