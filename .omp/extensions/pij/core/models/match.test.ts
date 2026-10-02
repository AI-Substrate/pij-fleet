// pij-control-plane — fuzzy model-id match tests (T001, TDD RED first).

import { describe, expect, it } from "vitest";
import { closestModel, normalizeModelQuery } from "./match.js";
import type { ModelEntry } from "./registry.js";

const MODELS: ModelEntry[] = [
	{ id: "fugu", name: "Sakana Fugu", provider: "sakana", verified: true },
	{ id: "fugu-ultra", name: "Sakana Fugu Ultra", provider: "sakana", verified: true },
	{ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6", provider: "claude", verified: false },
	{ id: "claude-opus-4-8", name: "Claude Opus 4.8", provider: "claude", verified: false },
	{ id: "gpt-4o", name: "GPT-4o", provider: "codex", verified: false },
	{
		id: "text-only-code-1",
		name: "Text-Only Code 1",
		provider: "copilot",
		verified: true,
	},
];

describe("normalizeModelQuery", () => {
	it("lowercases and replaces spaces with hyphens", () => {
		expect(normalizeModelQuery("Fugu Ultra")).toBe("fugu-ultra");
	});
	it("replaces underscores with hyphens", () => {
		expect(normalizeModelQuery("claude_sonnet_4_6")).toBe("claude-sonnet-4-6");
	});
	it("collapses multiple separators", () => {
		expect(normalizeModelQuery("fugu  ultra")).toBe("fugu-ultra");
	});
	it("trims whitespace", () => {
		expect(normalizeModelQuery("  gpt-4o  ")).toBe("gpt-4o");
	});
});

describe("closestModel", () => {
	it("'fugu ultra' matches fugu-ultra (space→hyphen normalisation)", () => {
		const r = closestModel("fugu ultra", MODELS);
		expect(r?.id).toBe("fugu-ultra");
	});

	it("exact id match wins", () => {
		const r = closestModel("fugu", MODELS);
		expect(r?.id).toBe("fugu");
	});

	it("case-insensitive match", () => {
		const r = closestModel("GPT-4O", MODELS);
		expect(r?.id).toBe("gpt-4o");
	});

	it("prefix match finds best candidate", () => {
		const r = closestModel("claude-opus", MODELS);
		expect(r?.id).toBe("claude-opus-4-8");
	});

	it("returns null for empty models list", () => {
		expect(closestModel("anything", [])).toBeNull();
	});

	it("returns null when nothing close enough", () => {
		// Query completely unlike anything in the list
		const r = closestModel("zzzzzzzzz", MODELS);
		expect(r).toBeNull();
	});
});
