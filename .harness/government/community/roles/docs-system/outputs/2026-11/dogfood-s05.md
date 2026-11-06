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

## The PA's wake (quoted by pij-watchful-badger)

Received 18:39:49 from `pij-bg`, msg_id `role-queue-01a1b003-99d4-7423-9e02-be142f680a48`:

```
[pij role queue] role:s05-probe in project pij-fleet has no live holder. pij-wooden-lungfish sent request 01a1b003-99d4-7423-9e02-be142f680a48 (reply none); it waits on the role's queue.
Next, one of:
  assign a fresh seat: pij directory assign s05-probe <seat> --why "<why>" --project pij-fleet
Assigning delivers the queue to the new holder. pij never starts a seat itself.
```

It names the role, the requester, the request id and its reply mode, and gives a runnable assign line with `--project` filled in. Badger took no action, as asked.

- **F60. The wake gives one step under "Next, one of:", and the API doc promises two.** (low) With no previous holder there is nothing to revive, so only `assign` is offered. That's right, but "one of:" over a single option reads like a dropped line. The API doc (§ Send to a role) says the wake "names ... two runnable steps: revive the last holder ... or `pij directory assign`". Fix: say "Next:" when there's one step, and say in the doc that the revive step appears only when the role has had a holder.

## Did the drain deliver? Unknown (F56, F59)

The assign at 18:40:11 should have drained request `01a1b003-...` to me as its new holder. As of 18:45 no message with a `[pij role:s05-probe request ...]` header has reached this seat, though badger's 18:42 message did arrive, so delivery was working. Either the drain didn't deliver (the request is still queued, and stays in the PA digest), or a request delivered back to its own sender was dropped. The assign receipt doesn't say (F56), and there's no verb to look the request up (F59), so I can't tell which. Badger (the PA) can't tell either. No watchdog nudge has run since the probe (the last was 08:34), no nudge has ever shown a role-queue request, and `pij directory` shows s05-probe with my 18:40:11-18:40:18 lineage, `live: []` and no queue field. Its top-level `pending` holds holdings whose close hasn't been committed yet (plan 195), not queued requests. So neither the requester, the holder nor the PA can see whether request `01a1b003-...` is still queued. That strengthens F59: the queue needs a read verb, for the PA as much as for the sender.

**Answer (pm-001, read-only check of the store and daemon.log).** At 08:40:15Z daemon.log has `role request 01a1b003-... for pij-wooden-lungfish not drained: pij-wooden-lungfish: a seat cannot send to itself`. I was both the requester and the new holder, so the drain hit the self-send refusal. The store row stays `queued`, and the only trace is that log line.

- **F61. A drain to the requester itself fails silently and strands the request.** (medium) Owner: plan 212. Either deliver it to the holder as a note to self, or close it as "requester took the role", and say which in the assign receipt (F56). Until then, request `01a1b003-...` stays queued on s05-probe, and the next assign of s05-probe (or its retirement) has to deal with it.

F52 to F59 belong to plan 212 (role send follow-ups); F55 also lands in #141's round. pm-001 works around F52 by assigning `security/<pr>` and `performance/<pr>` holdings, so `role:security/<pr>` resolves.
