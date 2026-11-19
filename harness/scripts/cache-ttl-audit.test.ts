import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const SCRIPT = resolve(import.meta.dirname, "cache-ttl-audit.py");

/** Run `source` with the audit loaded as `audit`; its stdout is JSON. */
function runAudit(source: string): unknown {
	const result = spawnSync(
		"python3",
		[
			"-c",
			`
import importlib.util, json
spec = importlib.util.spec_from_file_location("audit", ${JSON.stringify(SCRIPT)})
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
${source}
`,
		],
		{ encoding: "utf8" },
	);
	expect(result.stderr).toBe("");
	expect(result.status).toBe(0);
	return JSON.parse(result.stdout);
}

describe("cache-ttl-audit (F214, review R256-001)", () => {
	it("scores the guard's disposition, never the cache-state view, as its errors", () => {
		const stats = runAudit(`
MIN = 60_000
def row(ctx, idle, ttl, actual):
    return {
        "busy": False, "large": ctx > audit.COLD_CONTEXT, "actual": actual,
        "ctx": ctx, "idle_ms": idle, "new_ttl_ms": ttl,
        "old_guard": audit.old_guard(False, ctx, idle),
        "new_guard": audit.new_guard(False, ctx, idle, ttl, "a"),
        "old_hour_warm": idle <= audit.COLD_IDLE_MS,
        "old_cache": None if ttl not in (300_000, 3_600_000) else idle < ttl,
        "new_warm": None if ttl is None else idle < min(ttl, audit.COLD_IDLE_MS),
    }
rows = [
    row(100_000, 2 * MIN, 300_000, "warm"),    # small, clear, warm
    row(100_000, 20 * MIN, 300_000, "cold"),   # small, clear, cold: allowed cold, not large
    row(100_000, 20 * MIN, 300_000, "warm"),   # small past its TTL, warm: the view erred, the guard did not
    row(500_000, 8 * MIN, 300_000, "warm"),    # large past its own 5m TTL, warm: refused under (a)
    row(500_000, 10 * MIN, None, "warm"),      # large, TTL unknown: warned, warm
    row(500_000, 12 * MIN, 300_000, "cold"),   # large past 2 x its TTL, cold
]
print(json.dumps({"stats": audit.stats(rows), "replay": audit.replay(rows)}))
`);
		expect(stats).toMatchObject({
			stats: {
				decided: 6,
				guard_old_allowed_cold: 2,
				guard_old_allowed_cold_large: 1,
				guard_old_stopped_warm: 0,
				guard_old_refused: 0,
				guard_new_allowed_cold: 1,
				guard_new_allowed_cold_large: 0,
				guard_new_stopped_warm: 2,
				guard_new_refused: 2,
				guard_new_warned: 1,
				// The cache views' misses are their own columns, not guard errors.
				cache_new_said_cold_was_warm: 2,
				cache_new_ttl_unknown: 1,
				// The old cache view reads the seat's 5m/1h, not the old guard's hour.
				cache_old_said_warm_was_cold: 0,
				cache_old_said_cold_was_warm: 2,
				cache_old_ttl_unknown: 1,
				heuristic_old_hour_said_warm_was_cold: 2,
				heuristic_old_hour_said_cold_was_warm: 0,
			},
			replay: {
				a: {
					large_cold_allowed: 0,
					warm_refused_or_warned: 2,
					of_which_refused: 1,
					large_refusals: 2,
				},
				b: {
					large_cold_allowed: 1,
					of_which_warned: 1,
					warm_refused_or_warned: 2,
					large_refusals: 0,
				},
				"c x1.5": {
					large_cold_allowed: 0,
					warm_refused_or_warned: 2,
					of_which_refused: 1,
					large_refusals: 2,
				},
				"c x2": {
					large_cold_allowed: 0,
					warm_refused_or_warned: 2,
					of_which_refused: 0,
					large_refusals: 1,
				},
				"before F214": { large_cold_allowed: 1, warm_refused_or_warned: 0, large_refusals: 0 },
			},
		});
	});

	it("mirrors cold_wake::check's boundaries", () => {
		const verdicts = runAudit(`
MIN = 60_000
print(json.dumps([
    audit.new_guard(True, 900_000, 90 * MIN, 300_000),
    audit.new_guard(False, 400_000, 90 * MIN, None),
    audit.new_guard(False, 400_001, 5 * MIN - 1, 300_000),
    audit.new_guard(False, 400_001, 5 * MIN, 300_000, "a"),
    audit.new_guard(False, 400_001, 5 * MIN, 300_000),
    audit.new_guard(False, 400_001, 15 * MIN // 2 - 1, 300_000, "c x1.5"),
    audit.new_guard(False, 400_001, 15 * MIN // 2, 300_000, "c x1.5"),
    audit.new_guard(False, 400_001, 10 * MIN, 300_000, "c x2"),
    audit.new_guard(False, 400_001, 59 * MIN, 1_800_000, "b"),
    audit.new_guard(False, 400_001, 5 * MIN, None),
    audit.new_guard(False, 400_001, 5 * MIN + 1, None),
    audit.new_guard(False, 400_001, 60 * MIN + 1, 86_400_000),
    audit.old_guard(False, 400_001, 60 * MIN),
    audit.old_guard(False, 400_001, 60 * MIN + 1),
]))
`);
		expect(verdicts).toEqual([
			"busy",
			"clear",
			"clear",
			"cold",
			"warn",
			"warn",
			"cold",
			"cold",
			"warn",
			"clear",
			"warn",
			"cold",
			"clear",
			"cold",
		]);
	});
});
