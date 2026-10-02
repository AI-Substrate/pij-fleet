// pij-messaging — descriptor contract tests (plan 054 Phase 2, T001).
//
// Pins the WS-6 state vocabularies (human-ruled — an extend/rename must be a
// loud red here, never a silent drift: s055 consumes `systemState` by exact
// name) and the additive node-truth descriptor block. Guards follow the
// own-property law (platform/types.ts precedent): required fields must be OWN
// properties, prototype-supplied values are forgeries.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
	type ContextGauge,
	isContextGauge,
	isSemanticState,
	isSystemState,
	SEMANTIC_STATES,
	type SessionDescriptor,
	SYSTEM_STATES,
} from "./types.js";

describe("WS-6 vocabularies (human-ruled, exact)", () => {
	it("PIJ_ROLE narrowing accepts only Role words and discards any third word", () => {
		const indexSource = readFileSync(new URL("../index.ts", import.meta.url), "utf8");
		const narrowing = indexSource.match(
			/role = envRole === "([^"]+)" \|\| envRole === "([^"]+)" \? envRole : undefined;/,
		);
		expect(narrowing?.slice(1)).toEqual(["parent", "worker"]);
	});

	it("SEMANTIC_STATES is exactly the ruled 8-word semantic axis", () => {
		expect(SEMANTIC_STATES).toEqual([
			"blocked",
			"question",
			"hold",
			"waiting",
			"ready",
			"failed",
			"cancelled",
			"done",
		]);
	});

	it("SYSTEM_STATES is exactly the ruled 7-word mechanical axis", () => {
		expect(SYSTEM_STATES).toEqual([
			"starting",
			"working",
			"idle",
			"stalled",
			"stopped",
			"dead",
			"unknown",
		]);
	});

	it.each([...SEMANTIC_STATES])("isSemanticState accepts %s", (state) => {
		expect(isSemanticState(state)).toBe(true);
	});

	it.each([
		"working", // mechanical word — wrong axis
		"in-progress", // legacy SessionState word
		"Done", // case drift
		"",
		5,
		null,
		undefined,
		{},
	])("isSemanticState rejects %j", (value) => {
		expect(isSemanticState(value)).toBe(false);
	});

	it.each([...SYSTEM_STATES])("isSystemState accepts %s", (state) => {
		expect(isSystemState(state)).toBe(true);
	});

	it.each([
		"blocked", // semantic word — wrong axis
		"active", // legacy liveness word
		"stale", // legacy liveness word
		"pending", // lifecycle word
		"",
		0,
		null,
		undefined,
	])("isSystemState rejects %j", (value) => {
		expect(isSystemState(value)).toBe(false);
	});
});

describe("isContextGauge (honest-unknown law in the type)", () => {
	const valid: ContextGauge = {
		value: 123_456,
		asOf: "2026-07-17T00:00:00.000Z",
		provenance: "claude-transcript",
	};

	it("accepts a real numeric reading", () => {
		expect(isContextGauge(valid)).toBe(true);
	});

	it('accepts the honest "unknown" reading (copilot has no source)', () => {
		expect(isContextGauge({ ...valid, value: "unknown" })).toBe(true);
	});

	it.each([
		[null, "null value"],
		[Number.NaN, "NaN — a corrupted read is not a reading"],
		[Number.POSITIVE_INFINITY, "Infinity"],
		["12345", "stringly-typed number"],
		[undefined, "absent value"],
	])("rejects value %s (%s)", (value) => {
		expect(isContextGauge({ ...valid, value })).toBe(false);
	});

	it("rejects a gauge missing asOf or provenance", () => {
		const { asOf: _a, ...noAsOf } = valid;
		const { provenance: _p, ...noProvenance } = valid;
		expect(isContextGauge(noAsOf)).toBe(false);
		expect(isContextGauge(noProvenance)).toBe(false);
	});

	it.each([[null], [[]], ["gauge"], [42]])("rejects non-record %j", (value) => {
		expect(isContextGauge(value)).toBe(false);
	});

	it("own-property law: prototype-supplied fields are a forgery, not a gauge", () => {
		expect(isContextGauge(Object.create(valid))).toBe(false);
	});
});

describe("node-truth descriptor block (additive — AC-11 migration-safe)", () => {
	it("a descriptor carrying every node-truth field type-checks and round-trips values", () => {
		const full: SessionDescriptor = {
			id: "pij-full",
			folder: "/proj",
			dataDir: "/home/.pij/pij-full",
			eventsPath: "/home/.pij/pij-full/events.ndjson",
			pid: 4242,
			startedAt: "2026-07-17T00:00:00.000Z",
			currentAssignment: "asg-general-pij-full",
			currentTask: "review the fix packet",
			semanticState: "waiting",
			systemState: "idle",
			windowId: "@7",
			contextMax: 200_000,
			contextCurrent: {
				value: 88_000,
				asOf: "2026-07-17T00:00:01.000Z",
				provenance: "pi-inprocess",
			},
		};
		expect(full.semanticState).toBe("waiting");
		expect(full.systemState).toBe("idle");
		expect(full.windowId).toBe("@7");
		expect(full.contextMax).toBe(200_000);
		expect(full.contextCurrent?.value).toBe(88_000);
		expect(full.currentAssignment).toBe("asg-general-pij-full");
		expect(full.currentTask).toBe("review the fix packet");
	});

	it("a legacy descriptor with none of the new fields still type-checks (no new required fields)", () => {
		const legacy: SessionDescriptor = {
			id: "pij-legacy",
			folder: "/proj",
			dataDir: "/home/.pij/pij-legacy",
			eventsPath: "/home/.pij/pij-legacy/events.ndjson",
			pid: 1,
			startedAt: "2026-01-01T00:00:00.000Z",
		};
		expect(legacy.semanticState).toBeUndefined();
		expect(legacy.systemState).toBeUndefined();
		expect(legacy.contextCurrent).toBeUndefined();
	});
});
