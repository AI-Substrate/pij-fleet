// pij-control-plane — spawn warn-don't-block validation tests (T005).
//
// Validates that buildSpawnWarning produces a warning message for an unknown
// model but does NOT block spawn (the id is still returned immediately).

import { describe, expect, it } from "vitest";
import { buildEffortWarning, buildSpawnWarning } from "../../core/spawn.js";
import { type ModelEntry, modelsForRuntime, parseOmpModelsJson } from "./registry.js";

const KNOWN: ModelEntry[] = [
	{ id: "fugu-ultra", name: "Sakana Fugu Ultra", provider: "sakana", verified: true },
	{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", provider: "claude", verified: false },
];

describe("buildSpawnWarning", () => {
	it("returns null for a known model (no warning needed)", () => {
		expect(buildSpawnWarning("fugu-ultra", KNOWN)).toBeNull();
	});

	it("returns a warning string for an unknown model", () => {
		const w = buildSpawnWarning("gpt-99", KNOWN);
		expect(typeof w).toBe("string");
		expect(w).toContain("gpt-99");
	});

	it("includes the closest suggestion in the warning", () => {
		// "claude-sonnet" prefix-matches claude-sonnet-4-6 but isn't an exact id
		const w = buildSpawnWarning("claude-sonnet", KNOWN);
		expect(w).toContain("claude-sonnet-4-6");
	});

	it("returns null when model is undefined (no --model flag)", () => {
		expect(buildSpawnWarning(undefined, KNOWN)).toBeNull();
	});

	it("returns null when known list is empty (cannot validate — never block)", () => {
		expect(buildSpawnWarning("anything", [])).toBeNull();
	});

	it("warning does NOT include 'block' or 'abort' — spawn always proceeds", () => {
		const w = buildSpawnWarning("gpt-99-unknown", KNOWN);
		if (w) {
			expect(w.toLowerCase()).not.toMatch(/block|abort|fail|error/);
		}
	});
});

// ─── FIX-C mutation-proof: best-effort harness → no false "unknown model" ────
// Mutation: remove the `!known.some((e) => e.verified)` gate → buildSpawnWarning
// warns for claude/codex alias lists even when no entry is verified → RED.

describe("FIX-C: no false 'unknown model' warning for best-effort harness (INS-007)", () => {
	const UNVERIFIED: ModelEntry[] = [
		{ id: "claude-opus-4-8", name: "Claude Opus 4.8", provider: "claude", verified: false },
		{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", provider: "claude", verified: false },
	];

	it("returns null for any alias when all entries are unverified (cannot confirm absence)", () => {
		expect(buildSpawnWarning("sonnet", UNVERIFIED)).toBeNull();
	});

	it("returns null for a completely unknown name when all entries are unverified", () => {
		expect(buildSpawnWarning("gpt-99-fake", UNVERIFIED)).toBeNull();
	});

	it("still warns for a verified registry (pi) when a bogus model is given", () => {
		const piKnown: ModelEntry[] = [
			{ id: "fugu-ultra", name: "Sakana Fugu Ultra", provider: "sakana", verified: true },
		];
		const w = buildSpawnWarning("gpt-99-fake", piKnown);
		expect(typeof w).toBe("string");
		expect(w).toContain("gpt-99-fake");
	});
});

// ─── FIX-D mutation-proof: closest-match suggestion in the warning ────────────
// Mutation: remove `result.suggestion ? ` (did you mean '${result.suggestion}'?)` : ""`
// → suggestion absent from warning text → RED.

describe("FIX-D: warning for pi near-miss includes closest-match suggestion", () => {
	const PI_KNOWN: ModelEntry[] = [
		{ id: "fugu-ultra", name: "Sakana Fugu Ultra", provider: "sakana", verified: true },
		{ id: "fugu", name: "Sakana Fugu", provider: "sakana", verified: true },
	];

	it("warning includes the closest model suggestion for a near-miss (pi, verified)", () => {
		const w = buildSpawnWarning("fugu-ult", PI_KNOWN);
		expect(w).not.toBeNull();
		expect(w).toContain("fugu-ultra");
	});

	it("warning omits suggestion when nothing is close (no false suggestion)", () => {
		const w = buildSpawnWarning("zzz-nothing-like-this", PI_KNOWN);
		expect(w).not.toBeNull();
		expect(w).not.toContain("did you mean");
	});
});

describe("runtime-specific spawn validation (#306)", () => {
	const catalogs: ModelEntry[] = [
		{
			id: "gpt-5.6-sol-fast",
			selector: "gpt-5.6-sol-fast",
			requestModelId: "gpt-5.6-sol-fast",
			name: "Sol Fast",
			provider: "github-copilot",
			runtime: "copilot",
			verified: true,
			levels: ["none", "low", "medium", "high", "xhigh", "max"],
		},
		{
			id: "gpt-5.6-sol-fast-1m",
			selector: "github-copilot/gpt-5.6-sol-fast-1m",
			requestModelId: "gpt-5.6-sol-fast",
			name: "Sol Fast 1M",
			provider: "github-copilot",
			runtime: "omp",
			verified: true,
			levels: ["none", "low", "medium", "high", "xhigh", "max"],
		},
	];

	it("warns for an OMP-only selector on the Copilot CLI catalog", () => {
		const warning = buildSpawnWarning("gpt-5.6-sol-fast-1m", modelsForRuntime(catalogs, "copilot"));
		expect(warning).toContain("unknown model");
	});

	it("accepts the same selector on the OMP catalog", () => {
		expect(
			buildSpawnWarning("github-copilot/gpt-5.6-sol-fast-1m", modelsForRuntime(catalogs, "omp")),
		).toBeNull();
	});

	it("accepts a live-shape OMP effort without a false unsupported warning", () => {
		const omp = parseOmpModelsJson({
			models: [
				{
					provider: "openrouter",
					id: "reasoner",
					selector: "openrouter/reasoner",
					name: "Reasoner",
					thinking: ["off", "low", "medium", "high"],
				},
			],
		});
		expect(buildEffortWarning("high", "openrouter/reasoner", omp)).toBeNull();
	});
});
