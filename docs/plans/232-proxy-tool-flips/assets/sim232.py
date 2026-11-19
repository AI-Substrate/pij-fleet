"""Plan 232: re-price the fleet as if the proxied tool-list flips were gone, then weigh a per-seat cache TTL.

Extends egret's scratch/proxy-survey/sim.py. The windows and the egret-method pricing are his (sim.md, 03:20Z). Read-only:
it uses the usage fields of ~/.copilot/session-state/*/events.jsonl and nothing else.

Every Claude usage record carries its billed token classes (copilotUsage.tokenDetails: input, cache_read, cache_write
(5 min), cache_write_1h, output, each with count and price) and totalNanoAiu. So the counterfactual moves tokens
between classes and prices them at the record's own prices, and assumes no TTL the transcript does not record.

Counterfactual "fixed" (toolSearch=false, measured on canary session 23fd00ea and the prime's e6499e1f):
  1. Flip removed. A main-agent request whose tool list changed (the count, or the toolTokenCount with the count
     unchanged) reads the previous request's prompt instead of re-writing it. This applies only when the previous
     request ran in the same process, no compaction came between, and the gap is inside the previous request's
     cachePolicyTtlSeconds.
  2. Larger tool prefix. Each request's tool definitions grow to TOOLS_AFTER tokens. Tools lead the prompt, so the
     extra tokens are read when the request read any cache, and written at the request's own write price when it
     read none.
Plain (CAPI) records are not changed. Their re-pricing as proxied-fixed adds the same prefix growth,
d = TOOLS_AFTER - DELTA_BASE (the proxied before-window median tool size).

Usage: python3 sim232.py > sim232.md
"""
import collections
import datetime as dt
import glob
import json
import os
import statistics

HOME = os.path.expanduser("~")
FROM = "2026-11-11T05:55:00Z"
CUT = "2026-11-18T05:55:00Z"       # #243: the proxy went live
END = "2026-11-19T03:20:00Z"       # egret's run time, so the after-window matches sim.md
SETTING = "2026-11-19T03:27:46Z"   # user-wide toolSearch=false
TOOLS_AFTER = 30846                # toolTokenCount with toolSearch=false (opus 23fd00ea, haiku e6499e1f)
GAPS = ["<5 min", "5-60 min", ">60 min", "first"]
CLASSES = ["input", "cache_read", "cache_write", "cache_write_1h", "output"]


def secs(s):
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()


def gap_bucket(g):
    return "first" if g is None else "<5 min" if g < 5 else "5-60 min" if g < 60 else ">60 min"


def load():
    recs = []
    for path in glob.glob(f"{HOME}/.copilot/session-state/*/events.jsonl"):
        try:
            if os.path.getmtime(path) < secs(FROM):
                continue
            f = open(path, encoding="utf-8", errors="replace")
        except FileNotFoundError:  # session pruned between glob and open
            continue
        sid = path.split("/")[-2]
        start, compacted = None, False
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
                    continue
                if kind == "session.compaction_complete":
                    compacted = True
                    continue
                if kind != "session.usage_record" or ts < FROM:
                    continue
                u = e["data"]["usage"]
                model = str(u.get("model", ""))
                cu = u.get("copilotUsage") or {}
                n, P = cu.get("totalNanoAiu"), u.get("inputTokens") or 0
                if not model.startswith("claude") or not n or not P:
                    continue
                cls = {d["tokenType"]: (d.get("tokenCount") or 0, d["costPerBatch"] / d["batchSize"])
                       for d in cu.get("tokenDetails") or [] if d.get("tokenType") in CLASSES}
                if "output" not in cls:
                    continue
                agent = u.get("interactionType") == "conversation-agent"
                recs.append(dict(ts=ts, t=secs(ts), sid=sid, model=model, byok=bool(u.get("isByok")), P=P,
                                 actual=n, cls=cls, out_cost=cls["output"][0] * cls["output"][1],
                                 R=u.get("cacheReadTokens") or 0, W=u.get("cacheWriteTokens") or 0,
                                 ttl=u.get("cachePolicyTtlSeconds"), tools=u.get("availableToolCount"),
                                 tool_tokens=u.get("toolTokenCount"), start=start, agent=agent,
                                 after_compaction=compacted and agent))
                if agent:
                    compacted = False
    by = collections.defaultdict(list)
    for r in recs:
        by[r["sid"]].append(r)
    for rs in by.values():
        rs.sort(key=lambda r: r["t"])
        for i, r in enumerate(rs):  # egret's gap: since the session's previous Claude record of any kind
            r["gap"] = gap_bucket((r["t"] - rs[i - 1]["t"]) / 60 if i else None)
        prev = None
        for r in rs:  # the main agent's own stream, for flips and the TTL sim
            r["prev"] = prev if r["agent"] else None
            r["agent_gap"] = None if not r["agent"] or prev is None else (r["t"] - prev["t"]) / 60
            if r["agent"]:
                prev = r
    return recs, by


def class_cost(r, R, W):
    """Price a request with R read and W written tokens; fresh input and output as billed."""
    c = r["cls"]
    fresh = r["P"] - r["R"] - r["W"]
    if r["W"]:
        wprice = sum(c.get(k, (0, 0))[0] * c.get(k, (0, 0))[1] for k in ("cache_write", "cache_write_1h")) / r["W"]
    else:
        wprice = unit(r, "cache_write_1h" if r["ttl"] == 3600 else "cache_write")
    return fresh * unit(r, "input") + R * unit(r, "cache_read") + W * wprice + r["out_cost"]


def flip(r):
    p = r["prev"]
    return p is not None and (r["tools"] != p["tools"] or r["tool_tokens"] != p["tool_tokens"])


def fixable(r):
    p = r["prev"]
    return (r["byok"] and flip(r) and not r["after_compaction"] and p["start"] == r["start"]
            and p["ttl"] and r["agent_gap"] * 60 < p["ttl"])


def counterfactual(r, fix_flips=True, grow_prefix=True, delta_plain=0):
    """Return (P', cost') for one record under the fixed regime."""
    if not r["byok"]:
        d = delta_plain if grow_prefix else 0
        return r["P"] + d, None
    R, W = r["R"], r["W"]
    if fix_flips and r["agent"] and fixable(r):
        cached = min(r["P"] - (r["P"] - r["R"] - r["W"]), r["prev"]["P"])
        R, W = cached, r["R"] + r["W"] - cached
    d = max(0, TOOLS_AFTER - (r["tool_tokens"] or TOOLS_AFTER)) if grow_prefix else 0
    if R > 0:
        R += d
    else:
        W += d
    return r["P"] + d, class_cost(r, R, W)


def rates(recs, regime_of, cost_of, size_of):
    num, den, cnt = collections.Counter(), collections.Counter(), collections.Counter()
    for r in recs:
        g = regime_of(r)
        if g is None:
            continue
        k = (g, r["model"], r["gap"])
        num[k] += cost_of(r) - r["out_cost"]
        den[k] += size_of(r)
        cnt[k] += 1

    def rate(g, model, gap):
        k = (g, model, gap)
        if cnt[k] >= 20:
            return num[k] / den[k]
        n_ = sum(v for kk, v in num.items() if kk[0] == g and kk[2] == gap)
        d_ = sum(v for kk, v in den.items() if kk[0] == g and kk[2] == gap)
        return n_ / d_
    return rate, cnt


A = lambda n: n / 1e9  # nanoAiu -> AIU


def egret_tables(recs, label, delta_plain, **cf):
    """egret's method with a third regime, proxied-fixed (or a partial counterfactual)."""
    for r in recs:
        r["P_fix"], c = counterfactual(r, delta_plain=delta_plain, **cf)
        r["cost_fix"] = c if c is not None else r["actual"]
    regime = lambda r: "proxied" if r["byok"] else ("plain" if r["ts"] < CUT else None)
    base_rate, base_n = rates(recs, regime, lambda r: r["actual"], lambda r: r["P"])
    fix_rate, fix_n = rates([r for r in recs if r["byok"]], lambda r: "fixed", lambda r: r["cost_fix"],
                            lambda r: r["P_fix"])
    price = lambda rs, g: sum(r["P"] * base_rate(g, r["model"], r["gap"]) + r["out_cost"] for r in rs)
    price_fix = lambda rs: sum(r["P_fix"] * fix_rate("fixed", r["model"], r["gap"]) + r["out_cost"] for r in rs)
    out = [f"\n## {label}\n", "### Measured cache cost (AIU per 1M prompt tokens)\n",
           "| model | gap | plain (n) | proxied (n) | proxied-fixed (n) | fixed/plain |", "|---|---|---|---|---|---|"]
    for model in sorted({k[1] for k in base_n}):
        for gap in GAPS:
            a, b, c = ("plain", model, gap), ("proxied", model, gap), ("fixed", model, gap)
            if base_n[a] < 20 or base_n[b] < 20:
                continue
            pa, pb, pc = (base_rate(*a) * 1e6 / 1e9, base_rate(*b) * 1e6 / 1e9, fix_rate(*c) * 1e6 / 1e9)
            out.append(f"| {model} | {gap} | {pa:.1f} ({base_n[a]:,}) | {pb:.1f} ({base_n[b]:,}) | {pc:.1f} ({fix_n[c]:,}) | {pc / pa:.2f}× |")
    out += ["\n### The fleet, priced three ways\n",
            "| window | requests | billed AIU | as plain | as proxied | as proxied-fixed | proxied vs plain | proxied-fixed vs plain |",
            "|---|---|---|---|---|---|---|---|"]
    for name, sel in [("before #243 (7 d)", lambda r: r["ts"] < CUT), ("after #243 (to 03:20Z)", lambda r: CUT <= r["ts"] < END)]:
        rs = [r for r in recs if sel(r)]
        pl, px, pf = price(rs, "plain"), price(rs, "proxied"), price_fix(rs)
        out.append(f"| {name} | {len(rs):,} | {A(sum(r['actual'] for r in rs)):,.0f} | {A(pl):,.0f} | {A(px):,.0f} | {A(pf):,.0f} | "
                   f"{A(pl - px):+,.0f} ({1 - px / pl:+.1%}) | {A(pl - pf):+,.0f} ({1 - pf / pl:+.1%}) |")
    out += ["\n### By gap (both windows)\n", "| gap | requests | as plain | as proxied | as proxied-fixed | fixed − plain |", "|---|---|---|---|---|---|"]
    for gap in GAPS:
        rs = [r for r in recs if r["gap"] == gap and r["ts"] < END]
        pl, px, pf = price(rs, "plain"), price(rs, "proxied"), price_fix(rs)
        out.append(f"| {gap} | {len(rs):,} | {A(pl):,.0f} | {A(px):,.0f} | {A(pf):,.0f} | {A(pf - pl):+,.0f} |")
    return out


# ---------- per-seat TTL ----------
PRICES = collections.defaultdict(dict)  # model -> token class -> nanoAiu per token, from records that list it


def unit(r, key):
    got = r["cls"].get(key)
    return got[1] if got else PRICES[r["model"]][key]



def ttl_sim(sessions, policy, delta_plain, grow=True, flips=False):
    """Simulate the proxy's cache over each session's main-agent stream under a TTL policy.

    The cache is a list of segments [end, ttl_s, touched]. A request reads the longest prefix of live segments
    (a read refreshes a segment at its own TTL: a 1h marker over a 5m entry never upgrades it, per takin's spike).
    It writes the rest of its prompt as one new segment at the policy's TTL. A compaction clears the cache, and so
    does a tool-list change when flips=True (the calibration run against billed). Prices are the record's own."""
    total = collections.Counter()
    for rs in sessions:
        segs, history, prev_cacheable = [], [], None
        for r in rs:
            d = (max(0, TOOLS_AFTER - (r["tool_tokens"] or TOOLS_AFTER)) if r["byok"] else delta_plain) if grow else 0
            P = r["P"] + d
            fresh = r["P"] - r["R"] - r["W"]
            cacheable = P - fresh
            if r["after_compaction"] or (flips and flip(r)):
                segs = []
            hit = 0
            for s in segs:
                if r["t"] - s[2] < s[1] and s[0] <= cacheable:
                    hit = s[0]
                    s[2] = r["t"]
                else:
                    break
            segs = [s for s in segs if s[0] <= hit]
            ttl = policy(history, r)
            wtok = cacheable - hit
            if wtok > 0:
                segs.append([cacheable, ttl, r["t"]])
            wkey = "cache_write_1h" if ttl == 3600 else "cache_write"
            total["cost"] += (fresh * unit(r, "input") + hit * unit(r, "cache_read") + wtok * unit(r, wkey)
                              + r["out_cost"])
            total["n"] += 1
            total["writes_1h" if ttl == 3600 else "writes_5m"] += wtok
            if r["agent_gap"] is not None:
                history.append((r["agent_gap"], cacheable, prev_cacheable))
            prev_cacheable = cacheable
    return total


def hindsight(sessions, delta_plain):
    """Bound for any per-seat policy that holds one TTL per seat: each session takes whichever of 5m-for-all and
    1h-for-all was cheaper for it, known only afterwards."""
    total = 0
    for s in sessions:
        total += min(ttl_sim([s], lambda h, r: 300, delta_plain)["cost"], ttl_sim([s], lambda h, r: 3600, delta_plain)["cost"])
    return total


def last_n(n, k):
    return lambda h, r: 3600 if sum(1 for g, _, _ in h[-n:] if 5 <= g < 60) >= k else 300


def cost_based(n):
    """Pick the TTL that would have been cheaper over the seat's last n gaps, at opus's listed prices
    (read 20, 5m write 500, 1h write 800 AIU per 1M). Per-seat state the proxy would need: the time and prompt
    size of the last n requests, which it sees in each response's usage."""
    READ, W5, W1 = 20, 500, 800

    def pol(h, r):
        c1 = c5 = 0
        for g, cur, prev in h[-n:]:
            prev = prev or 0
            grow = max(0, cur - prev)
            c1 += prev * READ + grow * W1 if g < 60 else cur * W1
            c5 += prev * READ + grow * W5 if g < 5 else cur * W5
        return 3600 if c1 < c5 else 300
    return pol


def main():
    recs, by = load()
    for r in recs:
        for k, (_, p) in r["cls"].items():
            PRICES[r["model"]].setdefault(k, p)
    clash = {(r["model"], k) for r in recs for k, (_, p) in r["cls"].items() if p != PRICES[r["model"]][k]}
    if clash:
        print(f"WARNING: a token class has more than one price: {sorted(clash)}\n")
    before_tools = [r["tool_tokens"] for r in recs if r["byok"] and r["ts"] < SETTING and r["tool_tokens"]]
    delta_base = statistics.median(before_tools)
    delta_plain = TOOLS_AFTER - delta_base
    print(f"# Plan 232 sim: proxied fleet with the tool-list flips removed ({dt.datetime.now(dt.timezone.utc):%Y-%m-%dT%H:%MZ})\n")
    print(f"Windows as egret's sim.md: before = {FROM} to {CUT}, after = {CUT} to {END}. Claude records only; AIU = nanoAiu / 1e9.")
    print(f"Tool prefix: proxied before-window median toolTokenCount {delta_base:,.0f} → {TOOLS_AFTER:,} with toolSearch=false (+{delta_plain:,.0f}).\n")

    # calibration: token classes x their own prices against billed
    billed = sum(r["actual"] for r in recs)
    priced = sum(sum(cnt * p for cnt, p in r["cls"].values()) for r in recs)
    print(f"Calibration: token classes × listed prices = {A(priced):,.0f} AIU against billed {A(billed):,.0f} AIU over {len(recs):,} records.")
    px = [r for r in recs if r["byok"]]
    same = sum(class_cost(r, r["R"], r["W"]) for r in px)
    print(f"Calibration: proxied records re-priced unchanged by class_cost = {A(same):,.0f} AIU against billed {A(sum(r['actual'] for r in px)):,.0f}.\n")

    flips = [r for r in px if r["agent"] and flip(r)]
    fixed = [r for r in flips if fixable(r)]
    saved = sum(r["actual"] - counterfactual(r, grow_prefix=False)[1] for r in fixed)
    print(f"Proxied main-agent flips {len(flips):,}; fixable (same process, no compaction, inside TTL) {len(fixed):,}; "
          f"their re-writes cost {A(sum(r['actual'] for r in fixed)):,.0f} AIU billed, {A(saved):,.0f} AIU of which goes away.")

    for label, cf in [("Proxied-fixed: flips removed AND larger tool prefix (the real toolSearch=false)", {}),
                      ("Partial: flips removed only (no prefix growth)", {"grow_prefix": False}),
                      ("Partial: larger tool prefix only (flips kept)", {"fix_flips": False})]:
        print("\n".join(egret_tables(recs, label, delta_plain, **cf)))

    # live after-window by process start
    print("\n## Live window after the setting, by process start\n")
    print("| proxied requests | n | hot n | billed AIU | AIU per 1M prompt tokens (all) | AIU per 1M (hot) | flips |")
    print("|---|---|---|---|---|---|---|")
    for name, sel in [("before setting (to 03:27:46Z)", lambda r: CUT <= r["ts"] < SETTING),
                      ("after setting, process started before it", lambda r: r["ts"] >= SETTING and (r["start"] or "") < SETTING),
                      ("after setting, process started after it", lambda r: r["ts"] >= SETTING and (r["start"] or "") >= SETTING)]:
        rs = [r for r in px if sel(r)]
        hot = [r for r in rs if r["gap"] == "<5 min"]
        per = lambda xs: A(sum(r["actual"] - r["out_cost"] for r in xs)) / (sum(r["P"] for r in xs) / 1e6) if xs else 0
        print(f"| {name} | {len(rs):,} | {len(hot):,} | {A(sum(r['actual'] for r in rs)):,.0f} | {per(rs):.1f} | {per(hot):.1f} | "
              f"{sum(1 for r in rs if r['agent'] and flip(r))} |")

    # per-seat TTL
    sessions_all = [[r for r in rs if r["agent"] and r["ts"] < END] for rs in by.values()]
    sessions_all = [s for s in sessions_all if s]
    print("\n## Per-seat TTL, weighed on the fixed regime\n")
    print("Every Claude main-agent session in both windows (subagents excluded), simulated as a proxied seat with no flips and the larger tool prefix. "
          "Shared cross-seat prefix hits are not modelled (the same for every policy). The table shows cache and input cost; output is the same in every row and is included.\n")
    pols = [("5m for all", lambda h, r: 300), ("1h for all", lambda h, r: 3600)]
    for n, k in [(5, 1), (10, 1), (10, 2), (20, 2), (20, 3)]:
        pols.append((f"1h if ≥{k} of last {n} gaps were 5-60 min", last_n(n, k)))
    for n in (10, 20):
        pols.append((f"cost-based over last {n} gaps", cost_based(n)))
    windows = [("before #243 (7 d)", lambda r: r["ts"] < CUT), ("after #243 (to 03:20Z)", lambda r: r["ts"] >= CUT)]
    res = {}
    for wname, sel in windows:
        ss = [x for x in ([r for r in s if sel(r)] for s in sessions_all) if x]
        for name, pol in pols:
            res[(name, wname)] = ttl_sim(ss, pol, delta_plain)["cost"]
        res[("hindsight: each seat's cheaper fixed TTL (bound, not buildable)", wname)] = hindsight(ss, delta_plain)
    names = [n for n, _ in pols] + ["hindsight: each seat's cheaper fixed TTL (bound, not buildable)"]
    print("| policy | " + " | ".join(f"{w} AIU | vs 1h for all" for w, _ in windows) + " |")
    print("|---|" + "---|---|" * len(windows))
    for name in names:
        cells = []
        for wname, _ in windows:
            c, base = res[(name, wname)], res[("1h for all", wname)]
            cells.append(f"{A(c):,.0f} | {A(c - base):+,.0f} ({c / base - 1:+.1%})")
        print(f"| {name} | " + " | ".join(cells) + " |")

    # calibration of the TTL sim: the proxied main-agent sessions as they ran (1h, flips kept, no prefix growth)
    proxied = [x for x in ([r for r in s if r["byok"]] for s in sessions_all) if x]
    billed_px = sum(r["actual"] for s in proxied for r in s)
    as_ran = ttl_sim(proxied, lambda h, r: 3600, delta_plain, grow=False, flips=True)["cost"]
    no_flip = ttl_sim(proxied, lambda h, r: 3600, delta_plain, grow=False)["cost"]
    fixed = ttl_sim(proxied, lambda h, r: 3600, delta_plain)["cost"]
    print(f"\nTTL sim calibration on the proxied main-agent sessions to {END} ({sum(map(len, proxied)):,} requests): billed {A(billed_px):,.0f} AIU; "
          f"simulated as they ran (1h, flips kept) {A(as_ran):,.0f} ({as_ran / billed_px - 1:+.1%}); flips removed {A(no_flip):,.0f}; "
          f"flips removed and the larger prefix {A(fixed):,.0f} ({fixed / billed_px - 1:+.1%} against billed).")


if __name__ == "__main__":
    main()
