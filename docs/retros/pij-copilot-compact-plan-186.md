# Retro: plan 186, remote compact for Copilot seats (pij-copilot-compact)

Coder seat, Claude Opus 5.5, 2026-10-30. Parent: pij-far-jackal (pij-fleet prime).

## What shipped

`pij send <copilot-seat> --command compact` now compacts a Copilot seat. The
control queues on its own kind, `delivery-control:<seat>`, and the pointer drain
types `/compact` through the same staged-submit gate as Claude's compact. The
native extension keeps `delivery:<seat>` and never sees a control. `new` and
`reload` still refuse.

## magicWand

**A queue claim that filters on the row, not just its kind.** The plan was
written as "lift one refusal, route through the pane path". The real work was
discovering that a Copilot seat's control and body rows shared one kind, that
two independent claimers (the drain and the native extension) select by kind
alone, and that the only way to split them without touching the frozen
`ports.rs` was a second kind. That split then had to be threaded through
eleven `delivery:` SQL predicates, a shipped fake that mirrors them, and a
parking rule that turned out to stall behind the new kind. A `Queue::claim`
that took a typed row predicate would have made this a two-file change, and it
would have made "who may claim this row" a property you can read in one place
instead of inferring it from string prefixes scattered across the store.

## Difficulties

| # | severity | what | encoded? |
|---|---|---|---|
| 1 | high | **Mutant restore left stale build artifacts.** My mutant runner restored each file with `shutil.move` from a backup whose mtime predated the mutated build, so cargo kept the mutated object and the NEXT mutant ran on top of the previous one. Found when the unmutated branch failed the draft test 3/3 with a clean `git status`. All six receipts were voided and rerun with `os.utime` after restore. A green-that-lies cousin: the source tree is clean, the binary is not. | Fixed in my runner; candidate for a shared `harness mutate` helper that touches on restore. |
| 2 | medium | **Plan assumed Claude's compact "waits for idle".** It does not: the gate checks typing and an empty composer only, and Claude queues a typed `/compact` itself. Copilot's busy footer (`esc interrupt`) reads as an empty composer, so Copilot needed an explicit brake on its published `working` state. | Brake + test + mutant (M5). |
| 3 | medium | **Scope frozen before the shared-kind problem was visible.** Two questions to the prime (store scope, canary trust). Both were answered fast, but the first cost a context switch mid-implementation. | Raised at the point of discovery (see seam-change memory). |
| 4 | medium | **A canary on an isolated daemon cannot be driven without a human.** The isolated daemon accepts sends only from seats registered on it; the only seat is the canary itself, so its first prompt and Copilot's folder-trust dialog both needed the prime's hands. | Wishlist (`stretch:`): a daemon-owned canary sender (like `pij-bg`) for isolated test daemons. |
| 5 | medium | **`harness checks` cannot pass in a deps-less worktree.** The packet forbids `npm install`/`just worktree-deps`, so local-paths, lockfile, typecheck, lint, test and the trailer check all exit 127 on missing `node_modules`; my TS edits were proven only by CI. The gate reports these as `fail`, the same word as a real regression. | `stretch:` a `missing-prerequisite` verdict distinct from `fail`. |
| 6 | medium | **Copilot publishes `idle` between steps inside a turn** (2–4 ms `idle`→`working` pairs, seen live). The busy brake reads that state, so a drain pass landing in the gap would type mid-turn. None did in the canary. | Reported as residual risk; the fix is in the extension, outside this plan. |
| 7 | low | **A second daemon writes into `$HOME`.** Boot installs Claude hooks into every Claude home and `~/.pij-rs`; an isolated canary needs `HOME` pointed at scratch, which is not documented. | Noted in the canary evidence; candidate for a `--home`/`PIJ_RS_HOOK_DIR` flag. |
