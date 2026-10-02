// skills/flow-pair/test/paths.test.ts
// P8: tests target the lib directly.

import { describe, expect, it } from "vitest";
import { resolveRunDir } from "../lib/paths.js";

describe("resolveRunDir", () => {
	it("returns a path under ledgerRoot/runs/runId", () => {
		const result = resolveRunDir(".flow-pair", "r-2026-01");
		expect(result.ok).toBe(true);
		expect(result.runDir).toBe(".flow-pair/runs/r-2026-01");
	});

	it("works with absolute ledger root", () => {
		const result = resolveRunDir("/home/user/.flow-pair", "r-abc");
		expect(result.ok).toBe(true);
		expect(result.runDir).toBe("/home/user/.flow-pair/runs/r-abc");
	});

	it("is stable across repeated calls", () => {
		const r1 = resolveRunDir(".flow-pair", "run-1");
		const r2 = resolveRunDir(".flow-pair", "run-1");
		expect(r1.runDir).toBe(r2.runDir);
	});

	it("returns not-ok for empty runId", () => {
		const result = resolveRunDir(".flow-pair", "");
		expect(result.ok).toBe(false);
		expect(result.error).toBeTruthy();
	});

	it("rejects path-traversal runId containing '..'", () => {
		const result = resolveRunDir(".flow-pair", "../evil");
		expect(result.ok).toBe(false);
		expect(result.error).toBeTruthy();
	});

	it("rejects runId containing a path separator", () => {
		const result = resolveRunDir(".flow-pair", "a/b");
		expect(result.ok).toBe(false);
		expect(result.error).toBeTruthy();
	});

	it("rejects absolute runId", () => {
		const result = resolveRunDir(".flow-pair", "/absolute");
		expect(result.ok).toBe(false);
		expect(result.error).toBeTruthy();
	});

	it("rejects whitespace-only runId", () => {
		const result = resolveRunDir(".flow-pair", "   ");
		expect(result.ok).toBe(false);
		expect(result.error).toBeTruthy();
	});
});
