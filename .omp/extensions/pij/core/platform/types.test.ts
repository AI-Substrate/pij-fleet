// pij platform — T001 contract specs for core/platform/types (plan 054 Phase 1).
// RED by design: ./types.js does not exist yet — these specs pin the V-01
// binding contract before anything implements it. Records carry a numeric
// `schema_version: 1` (WS-4: the field is schema_version, never "version");
// SpineEvent `kind` is an OPEN string for external writers (WS-5) with pij's
// own kinds centralized as constants. Nothing throws (Pattern P4); guards are
// boolean predicates per the isFocusManifest precedent (adapters/focus-store.ts:26-49);
// slug/id helpers are total functions returning plain strings.

import { describe, expect, it } from "vitest";
import {
	type Assignment,
	type AttributionEnvelope,
	generalAssignmentId,
	isAssignment,
	isProject,
	isSpineEvent,
	isValidProjectSlug,
	kebabSlug,
	PROJECT_SLUG_MAX_LENGTH,
	type Project,
	resolveSlugCollision,
	SPINE_KIND_PROJECT_CREATED,
	SPINE_KIND_PROJECT_SET,
	SPINE_KIND_STATE_CLEARED,
	type SpineEvent,
} from "./types.js";

const TS = "2026-07-16T12:00:00.000Z";

function validProject(over: Partial<Project> = {}): Project {
	return {
		schema_version: 1,
		slug: "fix-the-cli",
		description: "Fix the CLI!",
		repo: "/repo/.git",
		planPath: "docs/plans/054-pij-grown-up/plan.md",
		primeId: "pij-witty-otter",
		created: { actor: "pij-witty-otter", ts: TS },
		...over,
	};
}

function validAssignment(over: Partial<Assignment> = {}): Assignment {
	return {
		schema_version: 1,
		id: "asg-general-pij-witty-otter",
		nodeId: "pij-witty-otter",
		projectSlug: "fix-the-cli",
		task: "hold the general assignment for this seat",
		states: [1, 4, 9],
		opened: { actor: "pij-witty-otter", ts: TS },
		closed: { actor: "pij-witty-otter", ts: TS, reason: "done" },
		...over,
	};
}

function validSpineEvent(over: Partial<SpineEvent> = {}): SpineEvent {
	return {
		schema_version: 1,
		seq: 7,
		ts: TS,
		actor: "pij-witty-otter",
		kind: "project-created",
		peer: "pij-calm-heron",
		project: "fix-the-cli",
		repo: "/repo/.git",
		refs: ["asg-general-pij-witty-otter"],
		prev: "6",
		next: "8",
		verifiedBy: "pij-daemon",
		actorProvenance: "resolved",
		...over,
	};
}

/** Copy a record with one key deleted (missing-required-field mutations). */
function without(record: object, key: string): Record<string, unknown> {
	const copy: Record<string, unknown> = { ...record };
	delete copy[key];
	return copy;
}

// ─── Project guard ──────────────────────────────────────────────────────────

describe("isProject", () => {
	it("accepts a fully-populated Project", () => {
		expect(isProject(validProject())).toBe(true);
	});

	it("accepts a minimal Project (repo/planPath/primeId absent)", () => {
		const minimal: Project = {
			schema_version: 1,
			slug: "fix-the-cli",
			description: "Fix the CLI!",
			created: { actor: "pij-witty-otter", ts: TS },
		};
		expect(isProject(minimal)).toBe(true);
	});

	it.each([
		"schema_version",
		"slug",
		"description",
		"created",
	])("rejects a Project missing required field %s", (key) => {
		expect(isProject(without(validProject(), key))).toBe(false);
	});

	it.each<[unknown]>([[0], [2], ["1"]])("rejects a Project with schema_version %j", (bad) => {
		expect(isProject({ ...validProject(), schema_version: bad })).toBe(false);
	});

	it.each<[string, unknown]>([
		["slug", 7],
		["description", null],
		["repo", 7],
		["planPath", false],
		["primeId", 9],
		["created", "not-an-object"],
	])("rejects a Project whose %s has the wrong type (%j)", (key, bad) => {
		expect(isProject({ ...validProject(), [key]: bad })).toBe(false);
	});

	it.each([
		"repo",
		"planPath",
		"primeId",
	])("rejects a Project whose optional %s is null (absent means undefined, never null)", (key) => {
		expect(isProject({ ...validProject(), [key]: null })).toBe(false);
	});

	it.each<[unknown]>([
		[{ ts: TS }], //                    missing actor
		[{ actor: "pij-witty-otter" }], //  missing ts
		[{ actor: 7, ts: TS }],
		[{ actor: "pij-witty-otter", ts: 7 }],
		[null],
		[["pij-witty-otter", TS]],
	])("rejects a Project with malformed created %j", (created) => {
		expect(isProject({ ...validProject(), created })).toBe(false);
	});

	it("tolerates unknown extra fields (additive / migration-safe records)", () => {
		expect(isProject({ ...validProject(), futureField: "x" })).toBe(true);
	});
});

// ─── Assignment guard ───────────────────────────────────────────────────────

describe("isAssignment", () => {
	it("accepts a fully-populated Assignment (opened + closed + projectSlug)", () => {
		expect(isAssignment(validAssignment())).toBe(true);
	});

	it("accepts a minimal Assignment (projectSlug and closed absent)", () => {
		const minimal: Assignment = {
			schema_version: 1,
			id: "asg-general-w3",
			nodeId: "w3",
			task: "general",
			states: [1],
			opened: { actor: "w3", ts: TS },
		};
		expect(isAssignment(minimal)).toBe(true);
	});

	it("accepts an empty states list (no spine refs yet)", () => {
		expect(isAssignment(validAssignment({ states: [] }))).toBe(true);
	});

	it.each([
		"schema_version",
		"id",
		"nodeId",
		"task",
		"states",
		"opened",
	])("rejects an Assignment missing required field %s", (key) => {
		expect(isAssignment(without(validAssignment(), key))).toBe(false);
	});

	it.each<[unknown]>([[0], [2], ["1"]])("rejects an Assignment with schema_version %j", (bad) => {
		expect(isAssignment({ ...validAssignment(), schema_version: bad })).toBe(false);
	});

	it.each<[string, unknown]>([
		["id", 7],
		["nodeId", 7],
		["projectSlug", 7],
		["task", 7],
		["states", "1,4"], //   not an array
		["states", [1, "4"]], // mixed entries
		["states", ["1"]], //   string seqs are refs done wrong
		["opened", "not-an-object"],
	])("rejects an Assignment whose %s has the wrong type (%j)", (key, bad) => {
		expect(isAssignment({ ...validAssignment(), [key]: bad })).toBe(false);
	});

	it("rejects an Assignment with projectSlug: null or states: null", () => {
		expect(isAssignment({ ...validAssignment(), projectSlug: null })).toBe(false);
		expect(isAssignment({ ...validAssignment(), states: null })).toBe(false);
	});

	it("rejects an Assignment whose states contain NaN (log refs, not free numbers)", () => {
		expect(isAssignment(validAssignment({ states: [Number.NaN] }))).toBe(false);
	});

	it("rejects array-like impostors for states (sparse array, TypedArray)", () => {
		const sparse = new Array<number>(2);
		sparse[1] = 4; // index 0 is a hole — .every would skip it
		expect(isAssignment({ ...validAssignment(), states: sparse })).toBe(false);
		expect(isAssignment({ ...validAssignment(), states: new Float64Array([1, 4]) })).toBe(false);
	});

	it.each<[unknown]>([
		[{ ts: TS }], //          missing actor
		[{ actor: "w3" }], //     missing ts
		[{ actor: 7, ts: TS }],
		[{ actor: "w3", ts: 7 }],
		[null],
	])("rejects an Assignment with malformed opened %j", (opened) => {
		expect(isAssignment({ ...validAssignment(), opened })).toBe(false);
	});

	it.each<[unknown]>([
		[null], //                                     closed must be absent, not null
		["done"],
		[{ ts: TS, reason: "done" }], //               missing actor
		[{ actor: "w3", reason: "done" }], //          missing ts
		[{ actor: "w3", ts: TS }], //                  missing reason
		[{ actor: "w3", ts: TS, reason: "aborted" }], // outside the 4-value union
		[{ actor: "w3", ts: TS, reason: 7 }],
		[{ actor: 7, ts: TS, reason: "done" }], //     all keys present, actor wrong-typed
		[{ actor: "w3", ts: 7, reason: "done" }], //   all keys present, ts wrong-typed
	])("rejects an Assignment with malformed closed %j", (closed) => {
		expect(isAssignment({ ...validAssignment(), closed })).toBe(false);
	});

	it.each([
		"done",
		"cancelled",
		"failed",
		"superseded",
	] as const)("accepts closed.reason '%s'", (reason) => {
		expect(isAssignment(validAssignment({ closed: { actor: "w3", ts: TS, reason } }))).toBe(true);
	});

	it("tolerates unknown extra fields (additive / migration-safe records)", () => {
		expect(isAssignment({ ...validAssignment(), futureField: "x" })).toBe(true);
	});
});

// ─── SpineEvent guard ───────────────────────────────────────────────────────

describe("isSpineEvent", () => {
	it("accepts a fully-populated SpineEvent", () => {
		expect(isSpineEvent(validSpineEvent())).toBe(true);
	});

	it("accepts a minimal SpineEvent (only required fields)", () => {
		const minimal: SpineEvent = {
			schema_version: 1,
			seq: 1,
			ts: TS,
			actor: "pij-witty-otter",
			kind: "project-created",
			refs: [],
		};
		expect(isSpineEvent(minimal)).toBe(true);
	});

	it("accepts an empty refs list (refs required, may be empty)", () => {
		expect(isSpineEvent(validSpineEvent({ refs: [] }))).toBe(true);
	});

	it.each([
		"schema_version",
		"seq",
		"ts",
		"actor",
		"kind",
		"refs",
	])("rejects a SpineEvent missing required field %s", (key) => {
		expect(isSpineEvent(without(validSpineEvent(), key))).toBe(false);
	});

	it.each<[unknown]>([[0], [2], ["1"]])("rejects a SpineEvent with schema_version %j", (bad) => {
		expect(isSpineEvent({ ...validSpineEvent(), schema_version: bad })).toBe(false);
	});

	it.each<[string, unknown]>([
		["seq", "3"], //      seq not number
		["ts", 7],
		["actor", 7],
		["kind", 7], //       open string, but still a string
		["refs", "a"], //     refs not an array
		["refs", [1]],
		["refs", ["a", 2]],
		["peer", 7],
		["project", 7],
		["repo", 7],
		["prev", 7],
		["next", 7],
		["verifiedBy", 7],
	])("rejects a SpineEvent whose %s has the wrong type (%j)", (key, bad) => {
		expect(isSpineEvent({ ...validSpineEvent(), [key]: bad })).toBe(false);
	});

	it.each([
		"peer",
		"project",
		"repo",
		"prev",
		"next",
		"verifiedBy",
	])("rejects a SpineEvent whose optional %s is null (absent means undefined, never null)", (key) => {
		expect(isSpineEvent({ ...validSpineEvent(), [key]: null })).toBe(false);
	});

	it("rejects a SpineEvent with refs: null", () => {
		expect(isSpineEvent({ ...validSpineEvent(), refs: null })).toBe(false);
	});

	it("rejects a SpineEvent with seq: NaN (typeof number is not enough for a log ref)", () => {
		expect(isSpineEvent({ ...validSpineEvent(), seq: Number.NaN })).toBe(false);
	});

	it("rejects a sparse refs array (holes that .every would skip)", () => {
		const sparse = new Array<string>(2);
		sparse[1] = "asg-general-pij-witty-otter"; // index 0 is a hole, reads as undefined
		expect(isSpineEvent({ ...validSpineEvent(), refs: sparse })).toBe(false);
	});

	it("accepts actorProvenance 'asserted' (and 'resolved' via the full fixture)", () => {
		expect(isSpineEvent(validSpineEvent({ actorProvenance: "asserted" }))).toBe(true);
	});

	it("accepts an absent actorProvenance", () => {
		expect(isSpineEvent(without(validSpineEvent(), "actorProvenance"))).toBe(true);
	});

	it.each<[unknown]>([
		["guessed"],
		[""],
		[7],
		[null],
	])("rejects actorProvenance %j (outside 'resolved'|'asserted')", (actorProvenance) => {
		expect(isSpineEvent({ ...validSpineEvent(), actorProvenance })).toBe(false);
	});

	it("tolerates unknown extra fields (additive / migration-safe records)", () => {
		expect(isSpineEvent({ ...validSpineEvent(), futureField: "x" })).toBe(true);
	});
});

// ─── cross-record foreignness ───────────────────────────────────────────────

describe("cross-record foreignness — a valid record of one shape fails the other guards", () => {
	it("a valid Project is not an Assignment", () => {
		expect(isAssignment(validProject())).toBe(false);
	});

	it("a valid Project is not a SpineEvent", () => {
		expect(isSpineEvent(validProject())).toBe(false);
	});

	it("a valid Assignment is not a Project", () => {
		expect(isProject(validAssignment())).toBe(false);
	});

	it("a valid Assignment is not a SpineEvent", () => {
		expect(isSpineEvent(validAssignment())).toBe(false);
	});

	it("a valid SpineEvent is not a Project", () => {
		expect(isProject(validSpineEvent())).toBe(false);
	});

	it("a valid SpineEvent is not an Assignment", () => {
		expect(isAssignment(validSpineEvent())).toBe(false);
	});
});

// ─── guards never throw (Pattern P4) ────────────────────────────────────────

describe("guards never throw", () => {
	it("returns false (a boolean, no throw) for every non-record input", () => {
		const inputs: unknown[] = [
			undefined,
			null,
			0,
			42,
			"project",
			true,
			Symbol("x"),
			() => {},
			[],
			[validProject()], // an array wrapping a valid record is still an array
		];
		for (const value of inputs) {
			expect(isProject(value)).toBe(false);
			expect(isAssignment(value)).toBe(false);
			expect(isSpineEvent(value)).toBe(false);
		}
	});

	it("returns false without throwing for a null-prototype record", () => {
		const bare: unknown = Object.create(null);
		expect(isProject(bare)).toBe(false);
		expect(isAssignment(bare)).toBe(false);
		expect(isSpineEvent(bare)).toBe(false);
	});
});

// ─── own-property law (review 001 F6) ───────────────────────────────────────
// The guards are trust boundaries for NON-JSON callers too: every required
// field must be an OWN property, and a known optional supplied only by the
// prototype chain is a forgery, not an absence. JSON.parse output (own data
// properties, incl. a literal "__proto__" key) must keep passing.

describe("own-property law (review 001 F6)", () => {
	it("Object.create(valid record) — every field inherited, none own — fails all three guards", () => {
		expect(isProject(Object.create(validProject()))).toBe(false);
		expect(isAssignment(Object.create(validAssignment()))).toBe(false);
		expect(isSpineEvent(Object.create(validSpineEvent()))).toBe(false);
	});

	it("a REQUIRED field supplied only by the prototype is rejected even when everything else is own", () => {
		const project = Object.assign(
			Object.create({ slug: "ghost" }),
			without(validProject(), "slug"),
		);
		expect(isProject(project)).toBe(false);
		const assignment = Object.assign(
			Object.create({ nodeId: "ghost" }),
			without(validAssignment(), "nodeId"),
		);
		expect(isAssignment(assignment)).toBe(false);
		const event = Object.assign(Object.create({ seq: 7 }), without(validSpineEvent(), "seq"));
		expect(isSpineEvent(event)).toBe(false);
	});

	it.each([
		[
			"project.repo",
			() => {
				const proto = { repo: "/injected/.git" };
				return isProject(Object.assign(Object.create(proto), without(validProject(), "repo")));
			},
		],
		[
			"project.primeId",
			() => {
				const proto = { primeId: "pij-intruder" };
				return isProject(Object.assign(Object.create(proto), without(validProject(), "primeId")));
			},
		],
		[
			"assignment.projectSlug",
			() => {
				const proto = { projectSlug: "injected" };
				return isAssignment(
					Object.assign(Object.create(proto), without(validAssignment(), "projectSlug")),
				);
			},
		],
		[
			"assignment.closed",
			() => {
				const proto = { closed: { actor: "x", ts: TS, reason: "done" } };
				return isAssignment(
					Object.assign(Object.create(proto), without(validAssignment(), "closed")),
				);
			},
		],
		[
			"spineEvent.prev",
			() => {
				const proto = { prev: "6" };
				return isSpineEvent(
					Object.assign(Object.create(proto), without(validSpineEvent(), "prev")),
				);
			},
		],
		[
			"spineEvent.actorProvenance",
			() => {
				const proto = { actorProvenance: "resolved" };
				return isSpineEvent(
					Object.assign(Object.create(proto), without(validSpineEvent(), "actorProvenance")),
				);
			},
		],
	])("a KNOWN optional supplied only by the prototype chain is rejected: %s", (_label, check) => {
		expect(check()).toBe(false);
	});

	it("nested stamps get the same law: prototype-backed created/opened/closed are rejected", () => {
		expect(isProject(validProject({ created: Object.create({ actor: "a", ts: TS }) }))).toBe(false);
		expect(isAssignment(validAssignment({ opened: Object.create({ actor: "a", ts: TS }) }))).toBe(
			false,
		);
		expect(
			isAssignment(
				validAssignment({ closed: Object.create({ actor: "a", ts: TS, reason: "done" }) }),
			),
		).toBe(false);
	});

	it("JSON.parse'd records still pass — including an own literal '__proto__' extra key", () => {
		expect(isProject(JSON.parse(JSON.stringify(validProject())))).toBe(true);
		expect(isAssignment(JSON.parse(JSON.stringify(validAssignment())))).toBe(true);
		expect(isSpineEvent(JSON.parse(JSON.stringify(validSpineEvent())))).toBe(true);
		// JSON.parse defines "__proto__" as an ordinary OWN data property (no
		// setter invocation) — an unknown extra, tolerated per AC-11.
		const withProto: unknown = JSON.parse(
			`{"__proto__":{"repo":"evil"},${JSON.stringify(validProject()).slice(1)}`,
		);
		expect(isProject(withProto)).toBe(true);
		expect(Object.hasOwn(withProto as object, "__proto__")).toBe(true);
	});
});

// ─── kind openness + pij spine-kind constants (WS-5) ────────────────────────

describe("spine kind openness + pij constants", () => {
	it("SPINE_KIND_PROJECT_CREATED is the literal 'project-created'", () => {
		expect(SPINE_KIND_PROJECT_CREATED).toBe("project-created");
	});

	it("SPINE_KIND_PROJECT_SET is the literal 'project-set'", () => {
		expect(SPINE_KIND_PROJECT_SET).toBe("project-set");
	});

	it("SPINE_KIND_STATE_CLEARED is the literal 'state-cleared'", () => {
		expect(SPINE_KIND_STATE_CLEARED).toBe("state-cleared");
	});

	it("isSpineEvent accepts arbitrary external kind strings (open vocabulary)", () => {
		expect(isSpineEvent(validSpineEvent({ kind: "weird-external-thing" }))).toBe(true);
	});

	it("isSpineEvent accepts both Phase-1 pij kinds", () => {
		expect(isSpineEvent(validSpineEvent({ kind: SPINE_KIND_PROJECT_CREATED }))).toBe(true);
		expect(isSpineEvent(validSpineEvent({ kind: SPINE_KIND_PROJECT_SET }))).toBe(true);
	});
});

// ─── generalAssignmentId ────────────────────────────────────────────────────

describe("generalAssignmentId", () => {
	it("returns exactly 'asg-general-<nodeId>'", () => {
		expect(generalAssignmentId("pij-witty-otter")).toBe("asg-general-pij-witty-otter");
	});

	it("templates any node id", () => {
		expect(generalAssignmentId("w3")).toBe("asg-general-w3");
	});

	it("returns a plain string for an empty nodeId (helpers never throw)", () => {
		expect(generalAssignmentId("")).toBe("asg-general-");
	});
});

// ─── slug rules (AC-01) ─────────────────────────────────────────────────────

describe("kebabSlug", () => {
	it("lowercases and kebabs the canonical example", () => {
		expect(kebabSlug("Fix the CLI!")).toBe("fix-the-cli");
	});

	it("collapses whitespace/punctuation runs to a single '-'", () => {
		expect(kebabSlug("Fix   the --- CLI!!  now")).toBe("fix-the-cli-now");
	});

	it("trims leading/trailing '-'", () => {
		expect(kebabSlug("  !!Fix the CLI!!  ")).toBe("fix-the-cli");
	});

	it("keeps digits", () => {
		expect(kebabSlug("Plan 054 grows up")).toBe("plan-054-grows-up");
	});

	it("returns '' for empty input", () => {
		expect(kebabSlug("")).toBe("");
	});

	it("returns '' for symbol-only input (a string, no throw)", () => {
		expect(kebabSlug("!!! --- ???")).toBe("");
	});

	it("is idempotent on an already-kebab slug", () => {
		expect(kebabSlug("fix-the-cli")).toBe("fix-the-cli");
	});
});

describe("resolveSlugCollision", () => {
	it("returns the base when free", () => {
		expect(resolveSlugCollision("fix-the-cli", new Set())).toBe("fix-the-cli");
	});

	it("suffixes -2 first when the base is taken (AC-01)", () => {
		expect(resolveSlugCollision("fix-the-cli", new Set(["fix-the-cli"]))).toBe("fix-the-cli-2");
	});

	it("walks -3, -4, … to the first free suffix", () => {
		const taken = new Set(["fix-the-cli", "fix-the-cli-2", "fix-the-cli-3"]);
		expect(resolveSlugCollision("fix-the-cli", taken)).toBe("fix-the-cli-4");
	});

	it("takes the earliest free suffix, not the first past all taken", () => {
		const taken = new Set(["fix-the-cli", "fix-the-cli-3"]);
		expect(resolveSlugCollision("fix-the-cli", taken)).toBe("fix-the-cli-2");
	});

	it("ignores unrelated taken slugs", () => {
		expect(resolveSlugCollision("fix-the-cli", new Set(["other", "fix-the-cli-2"]))).toBe(
			"fix-the-cli",
		);
	});
});

describe("isValidProjectSlug (s057 — explicit --slug shape)", () => {
	it("accepts strict kebab slugs", () => {
		for (const slug of ["fix-the-cli", "a", "plan-054", "a-b-c", "0-day"]) {
			expect(isValidProjectSlug(slug), slug).toBe(true);
		}
	});

	it("rejects non-kebab shapes (a string in, a boolean out — no throw)", () => {
		for (const slug of [
			"",
			"Fix-The-CLI", // uppercase
			"fix the cli", // spaces
			"-fix", // leading hyphen
			"fix-", // trailing hyphen
			"fix--cli", // double hyphen
			"fix_cli", // underscore
			"../escape", // path shape
		]) {
			expect(isValidProjectSlug(slug), slug).toBe(false);
		}
	});

	it("accepts exactly PROJECT_SLUG_MAX_LENGTH chars and rejects one more (boundary)", () => {
		const atMax = "a".repeat(PROJECT_SLUG_MAX_LENGTH);
		expect(isValidProjectSlug(atMax)).toBe(true);
		expect(isValidProjectSlug(`${atMax}a`)).toBe(false);
	});
});

// ─── AttributionEnvelope (compile-time contract) ────────────────────────────

describe("AttributionEnvelope", () => {
	// Type-level assertions: these consts fail `tsc` if the envelope shape drifts.
	const full: AttributionEnvelope = {
		actor: "pij-witty-otter",
		ts: TS,
		prev: "6",
		next: "8",
		refs: ["asg-general-pij-witty-otter"],
		verifiedBy: "pij-daemon",
		actorProvenance: "resolved",
	};
	const minimal: AttributionEnvelope = { actor: "pij-witty-otter", ts: TS, refs: [] };

	function attributionActor(envelope: AttributionEnvelope): string {
		return envelope.actor;
	}

	it("a full envelope satisfies the type and is structurally intact", () => {
		expect(attributionActor(full)).toBe("pij-witty-otter");
		expect(full).toMatchObject({ actor: "pij-witty-otter", ts: TS, actorProvenance: "resolved" });
	});

	it("a minimal envelope (actor/ts/refs only) satisfies the type", () => {
		expect(attributionActor(minimal)).toBe("pij-witty-otter");
		expect(minimal.refs).toEqual([]);
	});

	it("a SpineEvent is structurally accepted wherever an envelope is expected", () => {
		// Compile-time: SpineEvent is assignable to AttributionEnvelope.
		const event: SpineEvent = validSpineEvent();
		expect(attributionActor(event)).toBe("pij-witty-otter");
	});
});
