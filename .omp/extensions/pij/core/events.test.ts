import { describe, expect, it } from "vitest";

import { buildEvent, eventAgeMs, filterEvents, latestEventAgeMs } from "./events.js";
import type { PijEvent } from "./types.js";

const T0 = Date.parse("2026-06-16T00:00:00.000Z");

function ev(seq: number, type: string, offsetMs = 0, data?: unknown): PijEvent {
	return buildEvent(seq, type, T0 + offsetMs, data);
}

describe("buildEvent", () => {
	it("stamps seq + ISO-8601 timestamp", () => {
		const e = buildEvent(3, "tool_call", T0, { name: "ls" });
		expect(e.seq).toBe(3);
		expect(e.timestamp).toBe("2026-06-16T00:00:00.000Z");
		expect(e.type).toBe("tool_call");
		expect(e.data).toEqual({ name: "ls" });
	});

	it("omits data when undefined", () => {
		expect("data" in buildEvent(1, "state", T0)).toBe(false);
	});
});

describe("filterEvents", () => {
	const events = [ev(1, "tool_call"), ev(2, "tool_result"), ev(3, "tool_call"), ev(4, "message")];

	it("returns everything with no query", () => {
		expect(filterEvents(events)).toHaveLength(4);
	});
	it("filters by since (seq > since)", () => {
		expect(filterEvents(events, { since: 2 }).map((e) => e.seq)).toEqual([3, 4]);
	});
	it("filters by type", () => {
		expect(filterEvents(events, { type: "tool_call" }).map((e) => e.seq)).toEqual([1, 3]);
	});
	it("returns last N (present-minus-N)", () => {
		expect(filterEvents(events, { last: 2 }).map((e) => e.seq)).toEqual([3, 4]);
	});
	it("composes since → type → last", () => {
		expect(
			filterEvents(events, { since: 1, type: "tool_call", last: 1 }).map((e) => e.seq),
		).toEqual([3]);
	});
});

describe("age", () => {
	it("eventAgeMs = now − timestamp", () => {
		expect(eventAgeMs(ev(1, "message", 0), T0 + 5000)).toBe(5000);
	});
	it("latestEventAgeMs uses the highest-seq event", () => {
		const events = [ev(1, "message", 0), ev(3, "message", 2000), ev(2, "message", 1000)];
		expect(latestEventAgeMs(events, T0 + 3000)).toBe(1000);
	});
	it("latestEventAgeMs is null on an empty stream", () => {
		expect(latestEventAgeMs([], T0)).toBeNull();
	});
});
