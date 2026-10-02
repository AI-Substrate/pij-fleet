import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Value } from "typebox/value";
import { expect, it } from "vitest";
import activate from "./index.js";

it("pij_spawn requires an explicit known harness instead of inheriting pi", () => {
	let schema: Record<string, unknown> | undefined;
	activate({
		on: () => {},
		registerCommand: () => {},
		events: { on: () => {}, emit: () => {} },
		registerTool: (tool: { name: string; parameters: Record<string, unknown> }) => {
			if (tool.name === "pij_spawn") schema = tool.parameters;
		},
	} as unknown as ExtensionAPI);
	if (!schema) throw new Error("pij_spawn was not registered");
	expect(Value.Check(schema as never, {})).toBe(false);
	expect(Value.Check(schema as never, { harness: "unknown" })).toBe(false);
	for (const harness of ["omp", "pi", "claude", "copilot", "codex"]) {
		expect(Value.Check(schema as never, { harness })).toBe(true);
	}
});
