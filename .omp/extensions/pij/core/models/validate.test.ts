// pij-control-plane — validateModel + spawn warn-don't-block tests (T005).

import { describe, expect, it } from "vitest";
import { type ModelEntry, parseModelsJson } from "./registry.js";
import { type ValidationResult, validateEffort, validateModel } from "./validate.js";

const KNOWN: ModelEntry[] = [
	{ id: "fugu-ultra", name: "Sakana Fugu Ultra", provider: "sakana", verified: true },
	{ id: "fugu", name: "Sakana Fugu", provider: "sakana", verified: true },
	{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", provider: "claude", verified: false },
];

describe("validateModel", () => {
	it("returns ok for an exact known model id", () => {
		const r = validateModel("fugu-ultra", KNOWN);
		expect(r.ok).toBe(true);
	});

	it("returns ok for a case-insensitive match", () => {
		const r = validateModel("FUGU-ULTRA", KNOWN);
		expect(r.ok).toBe(true);
	});
	it("returns ok for an exact provider-qualified model id", () => {
		expect(validateModel("sakana/fugu-ultra", KNOWN).ok).toBe(true);
	});

	it("returns unknown=true for an unrecognised model id", () => {
		const r = validateModel("gpt-99", KNOWN);
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.unknown).toBe(true);
	});

	it("includes a suggestion when something close exists", () => {
		// "claude-sonnet" prefix-matches "claude-sonnet-4-6" but is not an exact id
		const r = validateModel("claude-sonnet", KNOWN) as Extract<ValidationResult, { ok: false }>;
		expect(r.ok).toBe(false);
		expect(r.suggestion).toBe("claude-sonnet-4-6");
	});

	it("suggestion is null when nothing is close", () => {
		const r = validateModel("zzz-nothing-like-this", KNOWN) as Extract<
			ValidationResult,
			{ ok: false }
		>;
		expect(r.ok).toBe(false);
		expect(r.suggestion).toBeNull();
	});

	it("returns ok for empty model string (no validation — spawn decides)", () => {
		// Empty model means no --model flag; nothing to validate
		const r = validateModel("", KNOWN);
		expect(r.ok).toBe(true);
	});

	it("returns ok when the known list is empty (cannot validate; never block)", () => {
		const r = validateModel("anything", []);
		expect(r.ok).toBe(true);
	});
});

// ─── Phase 2 (#3, task 2.6): validate --effort vs a model's levels ─────────────
// Warn-don't-block: only flag a positively-unsupported effort (model is known AND
// carries level data AND the effort is not among them). Everything else is ok.
const LEVELED: ModelEntry[] = [
	{
		id: "fugu",
		name: "Sakana Fugu",
		provider: "sakana",
		verified: true,
		reasoning: true,
		levels: ["high", "xhigh"],
	},
	{ id: "sonnet", name: "Claude Sonnet", provider: "claude", verified: false }, // no level data
];

const GPT56_LEVELS = ["none", "low", "medium", "high", "xhigh", "max"];
const GPT56_IDS = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"];
const GPT56_ENTRIES = parseModelsJson({
	providers: {
		"github-copilot": {
			models: GPT56_IDS.map((id) => ({
				id,
				reasoning: true,
				thinkingLevelMap: { xhigh: "xhigh", max: "max" },
			})),
		},
	},
});

describe("validateEffort", () => {
	it("ok when the effort is one of the model's levels (case-insensitive)", () => {
		expect(validateEffort("high", "fugu", LEVELED).ok).toBe(true);
		expect(validateEffort("XHIGH", "fugu", LEVELED).ok).toBe(true);
	});
	it("validates effort for an exact provider-qualified model id", () => {
		expect(validateEffort("xhigh", "sakana/fugu", LEVELED).ok).toBe(true);
		expect(validateEffort("medium", "sakana/fugu", LEVELED)).toEqual({
			ok: false,
			unsupported: true,
			levels: ["high", "xhigh"],
		});
	});

	it("unsupported when the effort is NOT in the model's levels (carries the levels)", () => {
		const r = validateEffort("medium", "fugu", LEVELED);
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.levels).toEqual(["high", "xhigh"]);
	});

	it("ok (cannot validate; never block) when the model is unknown", () => {
		expect(validateEffort("high", "totally-unknown", LEVELED).ok).toBe(true);
	});

	it("ok when the known model carries no level data", () => {
		expect(validateEffort("high", "sonnet", LEVELED).ok).toBe(true);
	});

	it("ok when no effort requested or no model given", () => {
		expect(validateEffort("", "fugu", LEVELED).ok).toBe(true);
		expect(validateEffort("high", undefined, LEVELED).ok).toBe(true);
	});

	it.each(GPT56_IDS)("validates the shared registry by exact bare id: %s", (id) => {
		expect(validateModel(id, GPT56_ENTRIES).ok).toBe(true);
	});

	it.each(
		GPT56_IDS.flatMap((id) => GPT56_LEVELS.map((level) => [id, level] as const)),
	)("accepts GPT-5.6 effort %s:%s", (id, level) => {
		expect(validateEffort(level, id, GPT56_ENTRIES).ok).toBe(true);
	});

	it.each(GPT56_IDS)("rejects unsupported minimal for %s with the exact level list", (id) => {
		expect(validateEffort("minimal", id, GPT56_ENTRIES)).toEqual({
			ok: false,
			unsupported: true,
			levels: GPT56_LEVELS,
		});
	});
});
