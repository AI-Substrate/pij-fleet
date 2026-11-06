# Dogfood note: s05 (send to a role, the role queue and reply modes)

docs-system (pij-wooden-lungfish), 2026-11-06 18:34 to 18:50 AEST, for pm-001. #129 merged at dc9f2e4; prod daemon pid 5893. Run from the linked worktree `fleet-docs-system` (detached at dc9f2e4). Receipts are kept in the docs-system seat's session files (`s05/`).

## 1. The receipt for pm-001's request (role:docs-system)

pm-001's message arrived with the header the API doc promises:

`[pij role:docs-system request 325693b633ea1aca-0000000000000005 from pij-steady-otter · reply wake]`

So the holder sees the role, the request id, the requester and the reply mode, with no limit (the role has no `default_limit`). One live holder (me), so it was delivered directly.

## 2. A role send to a one-holder role, `--reply fyi`

| Command | Result |
|---|---|
| `pij send role:pm-001 --reply fyi --body-file - <<'PIJ'` (the shim) | `sent (rs) — msg 325693b633ea1aca-0000000000000018 — receipt queued — extension-stream — cold-check: busy`, exit 0, 2.29 s |

Delivered to pij-steady-otter. The shim's line is the ordinary send receipt: it doesn't say this was a role send, nor show the request, the reply mode or the holder (F54).

## 3. A role with two live holders (role:security)

| Command | Result |
|---|---|
| `pij send role:security "..."` | `E-RS-ROLE-HOLDERS role:security has 2 live holders in project pij-fleet: pij-wary-lynx, pij-alert-ibex; address one as role:security/<instance>, or send to the seat` |
| `pij-rs --json send --to role:security --body "..."` | the same, `details.code: E-RS-ROLE-HOLDERS`, `details.next: null`, exit 2 |
| `pij-rs --json send --to role:security/x --body "..."` | `E-RS-ROLE-ADDRESS role security has one holder and no instances; address it as role:security` |
| `pij send role:security --body "..."` | `unknown send flag --body` (the shim takes text or `--body-file`) |

It refuses and lists both seats, as specified. But for a one-holder role the advice loops (F52).

## 4. A role with no live holder (scratch role `s05-probe`)

Every role in pij-fleet has at least one open holding, so I added a scratch role. I warned the PA (pij-watchful-badger: don't act on the wake) and the prime first.

| Step | Command | Result |
|---|---|---|
| add | `pij directory add s05-probe --why "..." --title ... --kind specialist --lifecycle per-task --holders one --rule ... --packet ...` | commit `69f97ea`, 4 paths. The shim printed the raw JSON envelope data (F58) |
| send | `pij-rs --json send --to role:s05-probe --reply none --body "..."` | `outcome: queued`, reason `no live holder of role:s05-probe in project pij-fleet: queued as request 01a1b003-... until a seat is assigned`; `request: {state: queued, reply: none, holder: null, limit_ms: null}`; `woke: {seat: pij-watchful-badger, outcome: queued, cold_check: unknown}` |
| drain | `pij-rs --json directory assign s05-probe pij-wooden-lungfish --why "..."` | commit `339ba51`; `seat_role: worker`; `window: {name: pij-fleet:s05-probe, renamed: true}`; nothing in the receipt says a request was drained (F56) |
| release | `pij-rs --json directory release s05-probe pij-wooden-lungfish --why "..."` | commit `d619f78`; lineage keeps the holding with both whys, the harness and its session id |

**Does it queue? Yes.** The receipt says so in words and in `request.state`.

**Is the PA woken? Yes**, per the receipt (`woke.seat: pij-watchful-badger`). The wake text and its runnable steps are what badger received; see "Pending" below.

The scratch role stays in the directory, with no holder, until `pij directory retire` exists: it lands with #135 (plan 206), which isn't on main yet. I'll retire it then.

## What worked

- The holder's header line carries the role, request id, requester and reply mode.
- One live holder: a role send is just a send. No holders: queued, with the PA woken, all visible in the `--json` receipt.
- Two holders: refused, naming both seats.
- Assign and release write clean lineage (who, why, harness and session).
- Cost: the sends launched nothing beyond an ordinary send. Each directory write was one commit (ddocs +2, git +7 for the add).

## What hurt (F52 onward)

- **F52. The two-holder refusal's advice loops on a one-holder role.** (medium) `E-RS-ROLE-HOLDERS` says "address one as role:security/<instance>", and `role:security/<anything>` refuses `E-RS-ROLE-ADDRESS ... has one holder and no instances; address it as role:security`. `details.next` is null. The only way through is "send to the seat", which is the last clause. Fix: for a one-holder role, the refusal names the two seats as `pij send <seat>` lines and says the role should have one holder (release one, F29); offer `/<instance>` only for a many-holder role.
- **F53. `pij send --help` (the shim) doesn't mention role sends.** (low) No `role:<id>`, `--reply`, `--limit` or `--project`; only `pij-rs send --help` documents them. Fix: add the role form and its three flags to the shim's help.
- **F54. The shim's receipt for a role send looks like any other send.** (low) `sent (rs) — msg ... — receipt queued` gives no request id, role, reply mode, state or holder; only `--json` shows `request` and `woke`. A queued request should say so in human output, e.g. `queued request <id> for role:<id>; woke <pa seat>`.
- **F55. The shim refuses `--body` on `send`; `pij-rs send` needs `--to` and `--body`.** (low) `pij send role:security --body "..."` gives `unknown send flag --body`. So a printed command like #141's `call_on` (`pij send --to role:<id> --body "..."`) doesn't run through the shim. Fix: the shim accepts `--to` and `--body` as aliases, or every printed command uses one CLI's form.
- **F56. Assign's receipt doesn't say what it drained.** (medium) The assign that should drain a queued request returns `commit, flushed, path, project, role, row, seat, seat_role, window`. (`flushed` is pending close-ends, not the queue.) The PA or prime who assigns can't see that the waiting requests were delivered, or to whom. Fix: `drained: [{request, from}]` in the receipt, and one human line per drained request.
- **F57. Assigning renames the holder's window even when it holds another role, and release doesn't put it back.** (low) My window became `pij-fleet:s05-probe` and stayed so after the release; I renamed it back to `pij-fleet:docs-system` by hand. This is F17 again, plus the missing restore. Fix with F17 (plan 208): name the window after the seat's lasting role, and restore it when a holding ends.
- **F58. `pij directory add` through the shim prints raw JSON.** (low) Same family as F42 and F50.
- **F59. A queued request can't be listed or withdrawn by its sender.** (medium) There is no `pij requests` or cancel verb. A mistaken or probe request can only be cleared by assigning a holder (which delivers it to that seat), and until then it sits in the PA digest. Fix: `pij requests [--role <id>] [--mine]` and `pij request withdraw <id> --why`, which marks it withdrawn and drops it from the digest.

## Info

- `pij-rs send` ids are UUIDs (`01a1b003-99d4-7423-...`), while the shim's are `<hex16>-<seq>` (`325693b633ea1aca-0000000000000018`). Both work as request ids. Noted only because the formats differ.

## Pending

- The PA's wake text (asked badger to quote it) and confirmation that the drained request reached me at my next turn. I'll add both here when they arrive.
