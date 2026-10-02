// pij platform — T003 contract specs for core/platform/spine (plan 054 Phase 1).
// These specs pin the pure spine-event surface. buildSpineEvent is fallible
// on the clock ONLY (review 001 F7 — E-ARG on NaN/out-of-TimeClip nowMs):
// it stamps schema_version 1 and ts = ISO(nowMs) (Pattern P3, injected
// clock) and defaults refs to []; it returns a SpineEventDraft —
// NO seq, the log port allocates it on append (review 001 F1). Stamping a
// seq onto a draft must yield a valid SpineEvent. filterSpineEvents mirrors
// core/events.ts filterEvents (events.ts:21-35): sequential narrowing
// since (seq > since, EXCLUSIVE) → peer → project, exact string equality
// (AC-02 — never prefix/substring), order preserved, input untouched.

import { describe, expect, it } from "vitest";
import { buildSpineEvent, filterSpineEvents } from "./spine.js";
import {
	isSpineEvent,
	SPINE_KIND_PROJECT_CREATED,
	type SpineEvent,
	type SpineEventDraft,
} from "./types.js";

const TS = "2026-07-16T12:00:00.000Z";
const NOW_MS = Date.parse(TS);

/** Fixture line builder — absent optionals stay ABSENT (never null). */
function ev(over: Partial<SpineEvent> & { seq: number }): SpineEvent {
	return {
		schema_version: 1,
		ts: TS,
		actor: "pij-witty-otter",
		kind: "state",
		refs: [],
		...over,
	};
}

/** Unwrap a valid-clock build (review 001 F7): every draft-shape spec
 *  funnels through here; the clock-failure paths are pinned separately. */
function draftOf(input: Parameters<typeof buildSpineEvent>[0]): SpineEventDraft {
	const result = buildSpineEvent(input);
	if (!result.ok) throw new Error(`${result.code}: ${result.message}`);
	return result.value;
}

// ─── buildSpineEvent — stamps + defaults (fallible on the clock only) ───────

describe("buildSpineEvent", () => {
	it("stamps schema_version 1, actor, kind, ts = ISO(nowMs); refs defaults to []", () => {
		const e: SpineEventDraft = draftOf({
			nowMs: NOW_MS,
			actor: "pij-witty-otter",
			kind: SPINE_KIND_PROJECT_CREATED,
		});
		expect(e).toMatchObject({
			schema_version: 1,
			ts: TS,
			actor: "pij-witty-otter",
			kind: SPINE_KIND_PROJECT_CREATED,
			refs: [],
		});
		expect(isSpineEvent({ ...e, seq: 7 })).toBe(true);
	});

	it("returns a draft with NO seq key — the log port allocates seq (review 001 F1)", () => {
		// Regression pin: caller-stamped seq must never return; even an
		// undefined-valued seq key would be a contract break.
		const e = draftOf({ nowMs: NOW_MS, actor: "pij-witty-otter", kind: "state" });
		expect("seq" in e).toBe(false);
	});

	it("derives ts from the injected nowMs (not a fixed clock)", () => {
		const e = draftOf({
			nowMs: NOW_MS + 1234,
			actor: "pij-witty-otter",
			kind: "state",
		});
		expect(e.ts).toBe("2026-07-16T12:00:01.234Z");
	});

	it("leaves every optional undefined on minimal input; stamped draft passes isSpineEvent", () => {
		const e = draftOf({ nowMs: NOW_MS, actor: "pij-witty-otter", kind: "state" });
		expect(e.peer).toBeUndefined();
		expect(e.project).toBeUndefined();
		expect(e.repo).toBeUndefined();
		expect(e.prev).toBeUndefined();
		expect(e.next).toBeUndefined();
		expect(e.verifiedBy).toBeUndefined();
		expect(e.actorProvenance).toBeUndefined();
		expect(isSpineEvent({ ...e, seq: 7 })).toBe(true);
	});

	it("passes every optional field through verbatim; stamped draft passes isSpineEvent", () => {
		const e = draftOf({
			nowMs: NOW_MS,
			actor: "pij-witty-otter",
			kind: SPINE_KIND_PROJECT_CREATED,
			actorProvenance: "resolved",
			peer: "pij-calm-heron",
			project: "fix-the-cli",
			repo: "/repo/.git",
			refs: ["project:fix-the-cli"],
			prev: "7",
			next: "9",
			verifiedBy: "pij-daemon",
		});
		expect(e).toMatchObject({
			schema_version: 1,
			ts: TS,
			actor: "pij-witty-otter",
			kind: SPINE_KIND_PROJECT_CREATED,
			actorProvenance: "resolved",
			peer: "pij-calm-heron",
			project: "fix-the-cli",
			repo: "/repo/.git",
			refs: ["project:fix-the-cli"],
			prev: "7",
			next: "9",
			verifiedBy: "pij-daemon",
		});
		expect(isSpineEvent({ ...e, seq: 8 })).toBe(true);
	});

	it("survives a JSON round-trip: parse(stringify(e)) equals e; stamped, passes isSpineEvent", () => {
		const minimal = draftOf({ nowMs: NOW_MS, actor: "pij-a", kind: "state" });
		const full = draftOf({
			nowMs: NOW_MS,
			actor: "pij-a",
			kind: SPINE_KIND_PROJECT_CREATED,
			actorProvenance: "asserted",
			peer: "pij-calm-heron",
			project: "fix-the-cli",
			repo: "/repo/.git",
			refs: ["project:fix-the-cli"],
			prev: "1",
			next: "3",
			verifiedBy: "pij-daemon",
		});
		for (const e of [minimal, full]) {
			const roundTripped = JSON.parse(JSON.stringify(e)) as SpineEventDraft;
			// No undefined-valued-key leakage worth breaking the on-disk guard.
			expect(roundTripped).toEqual(e);
			expect(isSpineEvent({ ...roundTripped, seq: 1 })).toBe(true);
		}
	});
});

// ─── filterSpineEvents — since → peer → project, exact, pure ───────────────

const e1 = ev({ seq: 1, peer: "pij-a", project: "fix" });
const e2 = ev({ seq: 2, peer: "pij-ab", project: "fix-the-cli" });
const e3 = ev({ seq: 3, peer: "a" });
const e4 = ev({ seq: 4, project: "fix" });
const e5 = ev({ seq: 5 });
const e6 = ev({ seq: 6, peer: "pij-a", project: "fix" });
const EVENTS: readonly SpineEvent[] = [e1, e2, e3, e4, e5, e6];

function seqs(events: readonly SpineEvent[]): number[] {
	return events.map((e) => e.seq);
}

describe("filterSpineEvents", () => {
	it("empty query returns every event as a fresh array (same element references)", () => {
		const omitted = filterSpineEvents(EVENTS);
		const explicit = filterSpineEvents(EVENTS, {});
		for (const out of [omitted, explicit]) {
			expect(out).toEqual([...EVENTS]);
			expect(out).not.toBe(EVENTS);
			expect(out[0]).toBe(e1);
		}
		// Mutating the copy never touches the input.
		omitted.push(e5);
		expect(EVENTS).toHaveLength(6);
	});

	it("since is EXCLUSIVE: keeps seq > since, never seq >= since", () => {
		expect(seqs(filterSpineEvents(EVENTS, { since: 2 }))).toEqual([3, 4, 5, 6]);
		expect(seqs(filterSpineEvents(EVENTS, { since: 0 }))).toEqual([1, 2, 3, 4, 5, 6]);
		expect(filterSpineEvents(EVENTS, { since: 6 })).toEqual([]);
	});

	it("peer matches exactly — 'pij-a' never matches 'pij-ab' or 'a' (AC-02)", () => {
		expect(seqs(filterSpineEvents(EVENTS, { peer: "pij-a" }))).toEqual([1, 6]);
	});

	it("peer 'a' never matches 'pij-a' or 'pij-ab' (no suffix/substring matching)", () => {
		expect(seqs(filterSpineEvents(EVENTS, { peer: "a" }))).toEqual([3]);
	});

	it("project matches exactly — 'fix' never matches 'fix-the-cli' and vice versa (AC-02)", () => {
		expect(seqs(filterSpineEvents(EVENTS, { project: "fix" }))).toEqual([1, 4, 6]);
		expect(seqs(filterSpineEvents(EVENTS, { project: "fix-the-cli" }))).toEqual([2]);
	});

	it("events with peer absent never match a peer filter", () => {
		expect(filterSpineEvents([e4, e5], { peer: "pij-a" })).toEqual([]);
	});

	it("events with project absent never match a project filter", () => {
		expect(filterSpineEvents([e3, e5], { project: "fix" })).toEqual([]);
	});

	it("filters AND-compose: since → peer → project narrow one result set", () => {
		expect(seqs(filterSpineEvents(EVENTS, { peer: "pij-a", project: "fix" }))).toEqual([1, 6]);
		expect(seqs(filterSpineEvents(EVENTS, { peer: "pij-a", project: "fix", since: 1 }))).toEqual([
			6,
		]);
		expect(seqs(filterSpineEvents(EVENTS, { peer: "pij-a", since: 5 }))).toEqual([6]);
		// AND, never OR: e2 has the project but the wrong peer.
		expect(filterSpineEvents(EVENTS, { peer: "pij-a", project: "fix-the-cli" })).toEqual([]);
	});

	it("preserves INPUT order — never re-sorts by seq", () => {
		const shuffled = [e6, e1, e4];
		expect(seqs(filterSpineEvents(shuffled, { project: "fix" }))).toEqual([6, 1, 4]);
	});

	it("never mutates the input array", () => {
		const input = [e2, e1, e3];
		const snapshot = [...input];
		filterSpineEvents(input, { since: 0, peer: "pij-a", project: "fix" });
		filterSpineEvents(input);
		expect(input).toEqual(snapshot);
		expect(input[0]).toBe(e2);
	});
});

// ─── review 001 F7 — the clock is checked, the constructor is fallible ─────

describe("buildSpineEvent rejects invalid clocks (review 001 F7)", () => {
	it("returns an ok Result carrying the draft for a valid clock", () => {
		const result = buildSpineEvent({ nowMs: NOW_MS, actor: "pij-witty-otter", kind: "note" });
		expect(result.ok).toBe(true);
		if (result.ok) {
			expect(result.value.ts).toBe(TS);
			expect("seq" in result.value).toBe(false);
		}
	});

	it.each([
		["NaN", Number.NaN],
		["Infinity", Number.POSITIVE_INFINITY],
		["one past TimeClip", 8.64e15 + 1],
	])("returns E-ARG naming nowMs for %s — never a RangeError", (_label, bad) => {
		const result = buildSpineEvent({ nowMs: bad, actor: "pij-witty-otter", kind: "note" });
		expect(result).toMatchObject({ ok: false, code: "E-ARG" });
		if (!result.ok) expect(result.message).toContain("nowMs");
	});
});
