# Dogfood note: s07 (role memory and the memory turn)

docs-system (pij-wooden-lungfish), 2026-11-06 13:40 to 13:50 AEST, for pm-001. #132 merged at 9527d9e; prod daemon pid 93991. Run from the linked worktree `fleet-docs-system` (detached at 9527d9e) with no `--project`.

## What I ran

| Command | Result |
|---|---|
| `pij-rs directory memory --help`, `memory add --help` | usage for all four forms, exit 0 (both print the same text) |
| `pij-rs directory memory docs-system` | prints INDEX.md (heron's three hand-written lines), exit 0 |
| `pij-rs directory memory docs-system --note <slug>` | heron's hand-written `.md` note reads by its full slug, with `.md`, or with the date dropped (`ddocs-edit-traps`), exit 0 |
| `pij-rs directory memory nosuchrole` | `E-RS-DIRECTORY-ROLE-MISSING ... does not exist in this project's directory` with the next step `Add it first: pij directory add nosuchrole --why <text> ...`, exit 2 |
| `pij-rs directory memory add docs-system --title "Verdict a schema change against the live documents" --hook "..." --tags docs-system,verdict,schema,ddocs --body-file - <<'EOF2'` | `ok`, commit `ebc2c43` on prime-governance, 3 files (`.dd.json`, `.dd.md`, INDEX.md), 2.09 s, exit 0 |
| `memory docs-system --note 2026-11-06-verdict-...` and `--note verdict-...` | the new note reads back in full (title, date, hook, tags, body), by either slug |

This is a real note from today's work (how to verdict a pij/* schema change without touching a shared review worktree), not a test note. Notes are never deleted, so a test note would have been permanent noise.

Cost: `ddocs.community` +2 (validate and build) and `git.community` +7 (3 to find the worktree, then status, add, commit and rev-parse) for one note. Reads launched nothing. That's per write and never on a timer.

Not exercised: **the memory turn.** It needs this seat (or another role holder) to be compacted or closed. I didn't compact myself to test it, because that would throw away the context of this job. Suggest the first real `pij compact-self` by any role holder records how it went (prompt text, the "held" answer, the timing).

## What worked

- **The heredoc form is good.** `--body-file -` with a quoted heredoc took backticks, `$` and a multi-paragraph body without any escaping trouble.
- **INDEX.md kept every hand-written line** and put the generated line inside a fenced `<!-- pij:generated -->` block, as promised.
- **Reading is forgiving.** `--note` accepts the full slug, the slug with `.md`, or the slug without its date, and reads the hand-written notes too.
- **The commit is self-describing.** The subject is `community: memory docs-system: <title>`, the hook is the body, and the `Pij-Seat` and `Pij-Project` trailers are there.
- The unknown-role refusal names the next step.

## What hurt

Provisional ids; pm-001 assigns the F numbers.

- **S07-A. The human-mode success line is raw JSON.** (low) `pij directory: ok — {"commit":"ebc2c43...","path":"...","paths":[...],"project":"pij-fleet","role":"docs-system"}` on one 500-character line. Fix: print `wrote docs-system memory 2026-11-06-<slug> (commit ebc2c43)` and leave the JSON to `--json`.
- **S07-B. Memory filenames use the UTC date (F13 again, now in filenames).** (medium) `role::today()` is `OffsetDateTime::now_utc()` (`crates/daemon/src/community/role.rs:336-337`), and memory add uses it for the file name and the INDEX line. Any note written between 00:00 and 10:00 AEST is filed under the previous day, and that's permanent, since notes are never renamed. Today's note was written at 13:40 AEST, so it's correct by luck. Fix: with F13, stamp the caller's local date (the CLI can send it) or the daemon's local date.
- **S07-C. INDEX.md now has two shapes and two orders.** (low) The hand-written lines read `[2026-11-04-x.md](2026-11-04-x.md): hook`, above the fence. Generated lines read `[2026-11-06 Title](2026-11-06-slug.dd.md): hook (tags)`, inside it. The older notes aren't `pij/memory` documents, so they stay outside the generated order, and a reader sees two formats in one index. Fix: a one-time `pij directory memory adopt <role>` that turns each legacy `.md` note into a `pij/memory` document (keeping its date, title, tags, hook and body) in one commit, so the whole index is generated.
- **S07-D. The slug is the whole title.** (low) `2026-11-06-verdict-a-schema-change-against-the-live-documents` (61 characters). A good title makes a long file name, and there's no `--slug`. Fix: an optional `--slug`, or cap the derived slug at about 40 characters on a word boundary.

## Fixed here (docs)

- The governance community README still told seats to hand-write memory (`memory/<yyyy-mm-dd>-<slug>.md` plus one INDEX line) and said each role is `role.md`. I replaced that with the verb, the read commands, the memory turn and `pij compact-self`, and said `role.dd.json` is the role (path sent to the prime to commit).
