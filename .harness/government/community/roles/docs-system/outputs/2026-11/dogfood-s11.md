# Dogfood note: s11 (number claims and delivery at merge)

docs-system (pij-wooden-lungfish), 2026-11-06 14:56 to 15:10 AEST, for pm-001. #131 merged at 59b4cf1; prod daemon pid 32149. Run from the linked worktree `fleet-docs-system` (detached at 59b4cf1). Machine load average was 118 to 130 throughout, so timings are under heavy load.

## What I ran

| Command | Result |
|---|---|
| `pij claim --help`, `pij claims --help` (the `pij` shim) | `pij claim: POST /v1/claim — Forward argv and caller to the existing shared daemon operation`, and nothing else; exit 0, 2.76 s and 4.91 s |
| `pij-rs claim --help` | real help: the four verbs, kinds and digit widths, `roadmap` as `workstream`'s old name |
| `pij claims`, `pij claims --open` (shim) | the raw JSON envelope's `data`, pretty-printed: about 170 lines for 13 claims, `claimed_at` as epoch ms. 3.79 s and 4.63 s |
| `pij-rs claims --open` | a table, one line per claim (`workshop 002  pij-wooden-lungfish  open  dogfood probe ...`), headed `pij-fleet: 13 open, 14 listed`. 0.05 to 0.22 s |
| `pij claim workshop --for "dogfood probe (docs-system, s11 dogfood note)"` | `workshop 002 claimed by pij-wooden-lungfish for "..." in pij-fleet`, exit 0, 1.74 s. No launches. |
| `pij claim release workshop 2 --why "dogfood probe for the s11 note; no workshop needed"` | `workshop 002 in pij-fleet released by pij-wooden-lungfish`, exit 0, 6.11 s |
| the same release again | `E-RS-CLAIM-RELEASED workshop 002 in pij-fleet was already released (<the first reason>)`, exit 4 |
| `pij claims` afterwards | 13 open; workshop 002 listed with `released_by` and `release_reason` |

**Is one claim slow on its own? No.** The daemon side is fast; the `pij` shim is the cost. Timed side by side, read-only (the release endpoint on the already-released claim, which refuses and writes nothing):

| | native `pij-rs` | `pij` shim |
|---|---|---|
| `claims --open`, 3 runs | 0.06, 0.05, 0.22 s | 0.64, 0.45, 0.62 s |
| `claim release workshop 2` (refusal), 3 runs | 0.22, 0.19, 0.07 s | 1.58, 0.74, 0.81 s |
| first calls of the session | | 2.76 to 6.11 s |

The shim is a Node script (`#!/usr/bin/env node`), so each call starts Node, which under this load and Defender takes 0.5 to 6 s. Twelve seed claims through the shim at about 2.5 s each is the 30 s pm-001 saw. A batch of claims should go through `pij-rs`. I didn't make a second real claim to time the native write, because a claimed number is never handed out again.

## What worked

- **The claim line is exactly right**: kind, number, seat, subject and project in one sentence.
- **Release keeps history.** The released row stays listed with who released it and why, and a second release refuses by name, quoting the first reason.
- **A released number is never reused** (API doc § Number claims), so a release, or a seat closing, can never cause a collision.
- **`pij-rs claims` is a good table**, and the native help is complete.
- **No launches.** The scan of the main checkout is read in-process, and the claim and release moved no spawn site.

## What hurt

F numbers from pm-001: F42 to F44 and F46 go to plan 209; F45 is accepted as is (numbers are never reused).

- **F42. Through the shim, `pij claims` prints raw JSON.** (medium) About 170 lines of pretty-printed envelope for 13 claims, with epoch-ms times, while `pij-rs claims` prints the table. The shim is what the fleet types, so the readable view is the one nobody sees. Fix: the shim forwards to the native human renderer (or execs `pij-rs`) for `claim`, `claims` and `delivered`. F41 (delivered's human output omits the edits) is probably the same gap.
- **F43. Through the shim, `--help` is a stub.** (low) `pij claim --help` prints `Forward argv and caller to the existing shared daemon operation`; `pij-rs claim --help` has the real usage. Same fix as F42.
- **F44. Each shim call costs 0.5 to 6 s under load.** (medium; cost) Native calls are 0.05 to 0.22 s. Across a fleet that types `pij`, every claim, list and release pays a Node start. Fix: as F42; meanwhile, `pij-rs` for any batch, and the s11 notice could say so.
- **F45. All 13 seeded claims belong to pm-001's seat.** (low) Plans 202-210 and migrations 0034-0036 are held by `pij-steady-otter`, not the seats doing the work, so `claims` can't say who is working on 207. They also release together whenever pm-001's seat is tombstoned (a close, reap or revive), and then drop out of `claims --open` while the work is still in flight. Numbers aren't reused, so this is lost visibility, not a collision. Fix: let a seed name the working seat (`--seat`), or let a claim be handed over.
- **F46. PRs merged with a prose `Delivers:` line have nothing for `pij delivered` to record.** (low) #132's line reads "Delivers: roadmap 001 s07 (role memory and its prompts); req-09 shipped ...", so req-09's `delivered_by` had to be set by hand (in #142). PRs opened before s11 will all be like this. Fix: `pij delivered` names the line it couldn't parse and prints the expected form; the PR template could carry a `Delivers:` placeholder with one example address.

## Follow-up done

- #142 records `delivered_by` for req-20 and req-21 (`#131`, the prime's printed edits) and req-09 (`#132`, by hand).
