"""Tool-list flips on proxied (BYOK) Copilot seats, before and after toolSearch=false (plan 232).

Reads every ~/.copilot/session-state/*/events.jsonl, read-only, and uses only the usage fields.
A request's *process start* is the latest session.start or session.resume event before it.
Copilot reads settings.json only at start, so a request belongs to the toolSearch=false regime only
when its process started at or after SETTING (the user-wide change, 03:27:46Z).

Per request: hot = under 5 min after the session's previous request; flip = availableToolCount differs
from the previous request in the same session; re-write share = cacheWriteTokens / inputTokens.
A request directly after a compaction is excluded from flip/re-write counts: compaction re-writes
the prompt for its own reason.

Usage: python3 flip_window.py [--session ID ...]   (with --session: per-request table for those)
"""
import collections
import datetime as dt
import glob
import json
import os
import sys

HOME = os.path.expanduser("~")
CUT = "2026-11-18T05:55:00Z"      # the proxy went live (#243), egret's after-window start
SETTING = "2026-11-19T03:27:46Z"  # ~/.copilot/settings.json toolSearch=false (prime, on Jordan's go)


def secs(s):
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()


def load(want=None):
    recs = []
    for path in glob.glob(f"{HOME}/.copilot/session-state/*/events.jsonl"):
        sid = path.split("/")[-2]
        if want and sid not in want:
            continue
        try:
            if not want and os.path.getmtime(path) < secs(CUT):
                continue
            f = open(path, encoding="utf-8", errors="replace")
        except FileNotFoundError:  # session pruned between glob and open
            continue
        start = None
        compacted = False
        with f:
            for line in f:
                if not any(k in line for k in ('"session.start"', '"session.resume"',
                                               '"session.compaction_complete"', '"session.usage_record"')):
                    continue
                try:
                    e = json.loads(line)
                except ValueError:
                    continue
                kind, ts = e.get("type"), e.get("timestamp", "")
                if kind in ("session.start", "session.resume"):
                    start = ts
                elif kind == "session.compaction_complete":
                    compacted = True
                elif kind == "session.usage_record":
                    u = e["data"]["usage"]
                    if not str(u.get("model", "")).startswith("claude") or ts < CUT:
                        continue
                    if u.get("interactionType") != "conversation-agent":
                        continue
                    recs.append(dict(sid=sid, ts=ts, t=secs(ts), start=start, model=u.get("model"),
                                     byok=bool(u.get("isByok")), tools=u.get("availableToolCount"),
                                     tool_tokens=u.get("toolTokenCount"), init=u.get("initiator"),
                                     P=u.get("inputTokens") or 0, R=u.get("cacheReadTokens") or 0,
                                     W=u.get("cacheWriteTokens") or 0, body=u.get("requestBodyBytes"),
                                     after_compaction=compacted))
                    compacted = False
    by = collections.defaultdict(list)
    for r in recs:
        by[r["sid"]].append(r)
    for rs in by.values():
        rs.sort(key=lambda r: r["t"])
        for i, r in enumerate(rs):
            p = rs[i - 1] if i else None
            r["gap"] = None if p is None else (r["t"] - p["t"]) / 60
            # A flip is any change to the tool definitions: the count, or (count unchanged) their
            # token size, which is what extensions_reload changes under toolSearch=false.
            r["count_flip"] = p is not None and r["tools"] != p["tools"]
            r["token_flip"] = p is not None and not r["count_flip"] and r["tool_tokens"] != p["tool_tokens"]
            r["flip"] = r["count_flip"] or r["token_flip"]
            r["delta"] = None if p is None or r["tools"] is None or p["tools"] is None else r["tools"] - p["tools"]
    return recs, by


def summary(recs):
    regimes = collections.OrderedDict([
        ("proxied, before (proxy live, started pre-setting, request pre-setting)",
         lambda r: r["byok"] and r["ts"] < SETTING),
        ("proxied, after, process started BEFORE setting (old seats)",
         lambda r: r["byok"] and r["ts"] >= SETTING and (r["start"] or "") < SETTING),
        ("proxied, after, process started AFTER setting (toolSearch=false)",
         lambda r: r["byok"] and r["ts"] >= SETTING and (r["start"] or "") >= SETTING),
        ("plain (CAPI), same span", lambda r: not r["byok"]),
    ])
    print("| regime | sessions | hot requests | tool count changed | tool tokens changed, count same | flip rate | of flips, re-wrote >50% | hot no-flip re-wrote >50% | hot write tokens per request | distinct tool counts |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for name, sel in regimes.items():
        rs = [r for r in recs if sel(r) and r["gap"] is not None and r["gap"] < 5 and not r["after_compaction"]]
        flips = [r for r in rs if r["flip"]]
        same = [r for r in rs if not r["flip"]]
        big = lambda xs: sum(1 for r in xs if r["P"] and r["W"] / r["P"] > 0.5)
        counts = collections.Counter(r["tools"] for r in rs)
        top = ", ".join(f"{k}:{v}" for k, v in counts.most_common(6))
        n = len(rs)
        print(f"| {name} | {len({r['sid'] for r in rs})} | {n:,} | {sum(r['count_flip'] for r in rs):,} | {sum(r['token_flip'] for r in rs):,} | {len(flips) / n if n else 0:.2%} | "
              f"{big(flips)} ({big(flips) / len(flips) if flips else 0:.0%}) | {big(same)} ({big(same) / len(same) if same else 0:.1%}) | "
              f"{sum(r['W'] for r in rs) / n if n else 0:,.0f} | {top} |")


def table(by, sids):
    for sid in sids:
        print(f"\n## {sid}\n")
        print("| ts | gap min | initiator | tools | Δ | toolTokenCount | input | cache read | cache write | write share | body bytes |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for r in by.get(sid, []):
            g = "first" if r["gap"] is None else f"{r['gap']:.1f}"
            d = "" if r["delta"] in (None, 0) else f"{r['delta']:+d}"
            print(f"| {r['ts'][11:19]} | {g} | {r['init']} | {r['tools']} | {d} | {r['tool_tokens']} | {r['P']:,} | {r['R']:,} | {r['W']:,} | "
                  f"{r['W'] / r['P'] if r['P'] else 0:.1%} | {r['body']:,} |")


if __name__ == "__main__":
    want = set(sys.argv[sys.argv.index("--session") + 1:]) if "--session" in sys.argv else None
    recs, by = load(want)
    if want:
        table(by, sorted(want))
    else:
        print(f"# Proxied tool-list flips, {CUT} to {dt.datetime.now(dt.timezone.utc):%Y-%m-%dT%H:%MZ} (setting {SETTING})\n")
        summary(recs)
