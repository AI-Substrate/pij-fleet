#!/usr/bin/env python3
"""F214 cache-TTL audit: pij's warm/cold verdict at delivery vs what the seat's
next model call actually did.

For every delivery pij made in a window (spine `delivery.outcome` = delivered),
it pairs the seat's previous main-chain call (idle time, context size) with its
next one (cacheRead share of the prompt: >= 50% warm, < 10% cold, between is
"partial" and set aside). It then reconstructs two verdicts at send time:

- `old`: the pre-F214 guard. Refuses a large (> 400k) seat only past 60 min,
  whatever its TTL. Its implicit "warm while idle <= 60 min" is scored as
  `heuristic_old_hour_*`; the pre-F214 cache-state view (5m/1h from the
  seat's record, anything else unknown) as `cache_old_*`.
- `new`: the F214 rule. The TTL comes from the seat's own record (Copilot's
  native cachePolicyTtlSeconds, or Claude/OMP's 1h/5m write split), then the
  proxy (proxied Copilot Claude: 1 h), then the table (crates/core
  session_status.rs CACHE_TTL_TABLE). Warm while idle < min(TTL, 60 min); a
  large seat past it is handled by PAST_TTL (shipped: warn until 60 min,
  refuse after; `replay` scores every value) and warned about past 5 min when
  no rung has a TTL.

A "silent cold wake" is a delivery the verdict let through with no word whose
next call re-wrote the cache. Read-only: it opens the store read-only and reads
transcripts; it contacts no daemon and launches nothing.

Usage:
  pij-rs fleet-report ~ --since 7d --threads 2 --out <dir>
  harness/scripts/cache-ttl-audit.py <dir> [--store ~/.pij-rs/pij.sqlite] [--json out.json]
The window is the fleet report's own (report.json).
"""

import argparse
import bisect
import json
import os
import sqlite3
from collections import defaultdict
from datetime import datetime

COLD_CONTEXT = 400_000
COLD_IDLE_MS = 3_600_000
UNKNOWN_TTL_WARN_MS = 300_000
NEXT_CALL_WITHIN_MS = 15 * 60_000
WARM_SHARE, COLD_SHARE = 0.5, 0.1
DAY_MS = 86_400_000
# Mirrors CACHE_TTL_TABLE v1: (harness, provider or None, family, seconds).
TABLE = [
    ("claude", None, "claude-", 3_600),
    ("copilot", None, "claude-", 300),
    ("copilot", None, "gpt-", 1_800),
    ("omp", "github-copilot", "claude-", 300),
    ("omp", "github-copilot", "gpt-", 1_800),
]


def iso_ms(text):
    return int(datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() * 1000)


def table_ttl(harness, model, seat_provider):
    prefix, _, name = model.rpartition("/")
    provider = prefix or seat_provider
    for row_harness, route, family, seconds in TABLE:
        if row_harness == harness and (route is None or route == provider) and name.startswith(family):
            return seconds * 1000
    return None


def load(args):
    report = json.load(open(os.path.join(args.report, "report.json")))
    since, until = report["window"]["since_ms"], report["window"]["until_ms"]
    db = sqlite3.connect(f"file:{os.path.expanduser(args.store)}?mode=ro", uri=True)
    deliveries = db.execute(
        "SELECT seat, at, json_extract(payload,'$.msg_id') FROM spine_events "
        "WHERE kind='delivery.outcome' AND at BETWEEN ? AND ? "
        "AND json_extract(payload,'$.outcome.outcome')='delivered' ORDER BY seq",
        (since, until),
    ).fetchall()
    pushed = dict(
        db.execute(
            "SELECT json_extract(payload,'$.msg_id'), at FROM spine_events "
            "WHERE kind='message.pushed' AND at BETWEEN ? AND ?",
            (since - DAY_MS, until),
        ).fetchall()
    )
    activity = defaultdict(list)
    for seat, at, state in db.execute(
        "SELECT seat, at, json_extract(payload,'$.state') FROM spine_events "
        "WHERE kind='seat.activity' AND at BETWEEN ? AND ? ORDER BY seq",
        (since - DAY_MS, until),
    ):
        activity[seat].append((at, state))
    seats = {
        row[0]: {"harness": row[1], "provider": row[2], "proxied": (row[3] or "").find('"proxied"') >= 0}
        for row in db.execute("SELECT id, harness, provider, cache_proxy FROM seats")
    }
    return since, until, deliveries, pushed, activity, seats


def calls_by_seat(args, since, until, seats):
    """seat -> sorted [(start_ms, model, prompt, cache_read, route, own_ttl_ms, ctx)]."""
    calls = defaultdict(list)
    sessions = [json.loads(line) for line in open(os.path.join(args.report, "tables/sessions.jsonl"))]
    main = {s["session"]: s for s in sessions if not s["is_sub"] and s["seat"]}
    for line in open(os.path.join(args.report, "tables/calls.jsonl")):
        c = json.loads(line)
        if c["harness"] not in ("claude-code", "oh-my-pi") or c["session"] not in main:
            continue
        prompt = c["input"] + c["cache_read"] + c["cw_5m"] + c["cw_1h"]
        if prompt == 0:
            continue
        own = 3_600_000 if c["cw_1h"] > 0 else 300_000 if c["cw_5m"] > 0 else None
        route = "anthropic" if c["harness"] == "claude-code" else (c["model"].rpartition("/")[0] or "omp")
        calls[c["seat"]].append((c["ts_ms"], c["model"], prompt, c["cache_read"], route, own, prompt))
    root = os.path.expanduser("~/.copilot/session-state")
    for s in main.values():
        if s["harness"] != "copilot-cli":
            continue
        path = os.path.join(root, s["session_id"], "events.jsonl")
        if not os.path.exists(path):
            continue
        for line in open(path):
            if '"session.usage_record"' not in line:
                continue
            event = json.loads(line)
            usage = event["data"]["usage"]
            if usage.get("interactionType") != "conversation-agent":
                continue  # subagent and utility calls hold their own caches
            start = iso_ms(event["timestamp"]) - int(usage.get("duration") or 0)
            if not since - DAY_MS <= start <= until + NEXT_CALL_WITHIN_MS:
                continue
            prompt = usage.get("inputTokens") or 0  # Copilot's input includes cache read and write
            if prompt == 0:
                continue
            ttl = usage.get("cachePolicyTtlSeconds")
            route = "copilot-proxied" if usage.get("isByok") else "copilot-plain"
            calls[s["seat"]].append(
                (start, usage["model"], prompt, usage.get("cacheReadTokens") or 0, route, ttl * 1000 if ttl else None, prompt)
            )
    for seat in calls:
        calls[seat].sort()
    return calls


def classify(args):
    since, until, deliveries, pushed, activity, seats = load(args)
    calls = calls_by_seat(args, since, until, seats)
    rows, seen = [], set()
    for seat, at, msg_id in deliveries:
        seat_calls = calls.get(seat)
        if not seat_calls:
            continue
        j = bisect.bisect_left([c[0] for c in seat_calls], at)
        if j == 0 or j >= len(seat_calls):
            continue
        prev, nxt = seat_calls[j - 1], seat_calls[j]
        if nxt[0] - at > NEXT_CALL_WITHIN_MS or (seat, nxt[0]) in seen:
            continue
        seen.add((seat, nxt[0]))
        send_at = pushed.get(msg_id, at)
        idle, ctx = max(0, send_at - prev[0]), prev[6]
        acts = activity.get(seat, [])
        k = bisect.bisect_right(acts, (send_at, "~")) - 1
        busy = k >= 0 and acts[k][1] == "working"
        info = seats.get(seat, {})
        harness = info.get("harness")
        # Old pij only knew 5m/1h; anything else (Copilot GPT's 1800 s) was unknown.
        old_ttl = prev[5] if prev[5] in (300_000, 3_600_000) else None
        if prev[5]:
            new_ttl, basis = prev[5], "own"
        elif info.get("proxied") and prev[1].rpartition("/")[2].startswith("claude-"):
            new_ttl, basis = 3_600_000, "proxy"
        else:
            new_ttl = table_ttl(harness, prev[1], info.get("provider"))
            basis = "table" if new_ttl else "unknown"
        large = ctx > COLD_CONTEXT
        share = nxt[3] / nxt[2]
        rows.append(
            {
                "seat": seat,
                "harness": harness,
                "model": prev[1].rpartition("/")[2],
                "route": prev[4],
                "at": send_at,
                "idle_ms": idle,
                "ctx": ctx,
                "large": large,
                "busy": busy,
                "old_guard": old_guard(busy, ctx, idle),
                "new_guard": new_guard(busy, ctx, idle, new_ttl),
                "old_hour_warm": idle <= COLD_IDLE_MS,
                "old_cache": None if old_ttl is None else idle < old_ttl,
                "new_ttl_ms": new_ttl,
                "ttl_basis": basis,
                "new_warm": None if new_ttl is None else idle < min(new_ttl, COLD_IDLE_MS),
                "share": round(share, 3),
                "actual": "warm" if share >= WARM_SHARE else "cold" if share < COLD_SHARE else "partial",
            }
        )
    return since, until, len(deliveries), rows


def old_guard(busy, ctx, idle):
    """The pre-F214 guard's disposition: `busy`, `cold` (refused) or `clear`."""
    if busy:
        return "busy"
    return "cold" if ctx > COLD_CONTEXT and idle > COLD_IDLE_MS else "clear"


# cold_wake::PastTtl values: None is Warn (refuse only past the hour), a
# number is RefuseAt { ttl_percent } as a multiple of the TTL.
POLICIES = {"a": 1.0, "b": None, "c x1.5": 1.5, "c x2": 2.0}
SHIPPED = "b"  # cold_wake::PAST_TTL


def new_guard(busy, ctx, idle, ttl, policy=SHIPPED):
    """cold_wake::check under a PAST_TTL value: `busy`, `cold`, `warn` or `clear`."""
    if busy:
        return "busy"
    if ctx <= COLD_CONTEXT:
        return "clear"
    if idle > COLD_IDLE_MS:
        return "cold"
    if ttl is None:
        return "warn" if idle > UNKNOWN_TTL_WARN_MS else "clear"
    if idle < ttl:
        return "clear"
    factor = POLICIES[policy]
    return "cold" if factor is not None and idle >= ttl * factor else "warn"


def replay(rows):
    """Every PAST_TTL value over the same decided deliveries: large cold sends
    let through (silently or with a warning), warm sends refused or warned, and
    large refusals."""
    decided = [r for r in rows if r["actual"] != "partial" and not r["busy"]]
    table = {}
    for policy in POLICIES:
        verdicts = [(r, new_guard(False, r["ctx"], r["idle_ms"], r["new_ttl_ms"], policy)) for r in decided]
        table[policy] = {
            "large_cold_allowed": sum(1 for r, v in verdicts if r["large"] and v != "cold" and r["actual"] == "cold"),
            "of_which_warned": sum(1 for r, v in verdicts if v == "warn" and r["actual"] == "cold"),
            "warm_refused_or_warned": sum(1 for r, v in verdicts if v in ("cold", "warn") and r["actual"] == "warm"),
            "of_which_refused": sum(1 for r, v in verdicts if v == "cold" and r["actual"] == "warm"),
            "large_refusals": sum(1 for _, v in verdicts if v == "cold"),
        }
    old = [(r, old_guard(False, r["ctx"], r["idle_ms"])) for r in decided]
    table["before F214"] = {
        "large_cold_allowed": sum(1 for r, v in old if r["large"] and v != "cold" and r["actual"] == "cold"),
        "of_which_warned": 0,
        "warm_refused_or_warned": sum(1 for r, v in old if v == "cold" and r["actual"] == "warm"),
        "of_which_refused": sum(1 for r, v in old if v == "cold" and r["actual"] == "warm"),
        "large_refusals": sum(1 for _, v in old if v == "cold"),
    }
    return table


def stats(rows):
    """Guard outcomes and cache-state views, in separately named columns
    (review R256-001), over decided deliveries (not busy, not partial).

    guard_*: what the cold-wake guard did with the send, from each rule's
    disposition: allowed in silence (`clear`) yet the next call was cold;
    refused (`cold`) or warned (`warn`) yet the next call read the cache.
    cache_*: what the cache-state view (`pij state`, the CACHE column, the FYI
    flush) said, for every size, over sends whose TTL that view knew; it
    decides no send. heuristic_old_hour_*: the old guard's fixed-hour idea of
    warm, which is no cache view.
    """
    decided = [r for r in rows if r["actual"] != "partial" and not r["busy"]]
    cold = lambda r: r["actual"] == "cold"
    warm = lambda r: r["actual"] == "warm"
    out = {"n": len(rows), "decided": len(decided), "actual_cold": sum(map(cold, decided))}
    for rule in ("old", "new"):
        verdict = lambda r, rule=rule: r[f"{rule}_guard"]
        out[f"guard_{rule}_allowed_cold"] = sum(1 for r in decided if verdict(r) == "clear" and cold(r))
        out[f"guard_{rule}_allowed_cold_large"] = sum(
            1 for r in decided if verdict(r) == "clear" and cold(r) and r["large"]
        )
        out[f"guard_{rule}_stopped_warm"] = sum(1 for r in decided if verdict(r) in ("cold", "warn") and warm(r))
        out[f"guard_{rule}_refused"] = sum(1 for r in decided if verdict(r) == "cold")
        out[f"guard_{rule}_warned"] = sum(1 for r in decided if verdict(r) == "warn")
    out["heuristic_old_hour_said_warm_was_cold"] = sum(1 for r in decided if r["old_hour_warm"] and cold(r))
    out["heuristic_old_hour_said_cold_was_warm"] = sum(1 for r in decided if not r["old_hour_warm"] and warm(r))
    for rule, key in (("old", "old_cache"), ("new", "new_warm")):
        known = [r for r in decided if r[key] is not None]
        out[f"cache_{rule}_said_warm_was_cold"] = sum(1 for r in known if r[key] and cold(r))
        out[f"cache_{rule}_said_cold_was_warm"] = sum(1 for r in known if not r[key] and warm(r))
        out[f"cache_{rule}_ttl_unknown"] = len(decided) - len(known)
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("report", help="a pij-rs fleet-report output folder")
    parser.add_argument("--store", default="~/.pij-rs/pij.sqlite")
    parser.add_argument("--json", help="write every paired row here")
    args = parser.parse_args()
    since, until, delivered, rows = classify(args)
    groups = defaultdict(list)
    for row in rows:
        groups[(row["harness"], row["model"], row["route"])].append(row)
    total = stats(rows)
    print(f"window {since}..{until}: {delivered} delivered, {len(rows)} paired, {total['decided']} decided")
    cols = list(total)
    print("| harness | model | route | " + " | ".join(cols) + " |")
    print("|" + "---|" * (len(cols) + 3))
    for key in sorted(groups, key=lambda k: -len(groups[k])):
        s = stats(groups[key])
        print("| " + " | ".join(str(k) for k in key) + " | " + " | ".join(str(s[c]) for c in cols) + " |")
    print("| TOTAL | | | " + " | ".join(str(total[c]) for c in cols) + " |")
    print("\nPAST_TTL replay over the decided deliveries (shipped: " + SHIPPED + ")")
    policies = replay(rows)
    cols = list(next(iter(policies.values())))
    print("| policy | " + " | ".join(cols) + " |")
    print("|" + "---|" * (len(cols) + 1))
    for policy, row in policies.items():
        print(f"| {policy} | " + " | ".join(str(row[c]) for c in cols) + " |")
    if args.json:
        with open(args.json, "w") as out:
            json.dump({"window": [since, until], "rows": rows, "total": total}, out)


if __name__ == "__main__":
    main()
