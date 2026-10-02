// pij platform — checked ISO timestamp specs (review 001 F7).
// isoTimestamp is the ONE place a clock becomes an ISO string: finite epoch
// milliseconds within the ECMA TimeClip range (|ms| ≤ 8.64e15) stamp cleanly;
// everything else is E-ARG naming nowMs — never a RangeError escaping the
// tagged-union contract. types.ts must never import this module (zero-import
// law) — that is pinned by the boundary sensor's covered-files list staying
// honest and by types.ts itself importing nothing.

import { describe, expect, it } from "vitest";
import { isoTimestamp } from "./time.js";

const MAX_TIME_MS = 8.64e15;

describe("isoTimestamp", () => {
	it("stamps a finite in-range clock as ISO-8601", () => {
		const t = Date.parse("2026-06-16T12:00:00.000Z");
		expect(isoTimestamp(t)).toEqual({ ok: true, value: "2026-06-16T12:00:00.000Z" });
	});

	it("stamps epoch zero", () => {
		expect(isoTimestamp(0)).toEqual({ ok: true, value: "1970-01-01T00:00:00.000Z" });
	});

	it("accepts the TimeClip boundary itself (±8.64e15 — Date's own limit)", () => {
		expect(isoTimestamp(MAX_TIME_MS)).toMatchObject({ ok: true });
		expect(isoTimestamp(-MAX_TIME_MS)).toMatchObject({ ok: true });
	});

	it.each([
		["NaN", Number.NaN],
		["Infinity", Number.POSITIVE_INFINITY],
		["-Infinity", Number.NEGATIVE_INFINITY],
		["one past TimeClip", MAX_TIME_MS + 1],
		["one before -TimeClip", -MAX_TIME_MS - 1],
	])("rejects %s with E-ARG naming nowMs — no throw", (_label, bad) => {
		const result = isoTimestamp(bad);
		expect(result).toMatchObject({ ok: false, code: "E-ARG" });
		if (!result.ok) expect(result.message).toContain("nowMs");
	});
});
