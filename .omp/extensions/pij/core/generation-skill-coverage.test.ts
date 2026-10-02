import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { RS_ROUTE_TABLE } from "./generation-routing.js";

const skill = readFileSync(new URL("../../../../skills/pij/SKILL.md", import.meta.url), "utf8");

function skillVerbs(markdown: string): string[] {
	const heading = /^## CLI-verb coverage\b.*$/m.exec(markdown);
	if (heading === null) throw new Error("Missing SKILL.md CLI-verb coverage section");
	const section = markdown.slice(heading.index + heading[0].length).split(/^## /m, 1)[0] ?? "";
	const verbs = new Set<string>();
	for (const line of section.split("\n")) {
		const cell = /^\|([^|]*)\|/.exec(line)?.[1];
		if (cell === undefined) continue;
		// Leaves and flags are annotations, not additional top-level CLI verbs.
		const topLevel = cell.replace(/\([^)]*\)/g, "");
		for (const match of topLevel.matchAll(/`([a-z][a-z0-9-]*)`/g)) {
			const verb = match[1];
			if (verb === undefined) throw new Error("Missing CLI verb regex capture");
			verbs.add(verb);
		}
	}
	if (verbs.size === 0) throw new Error("Empty SKILL.md CLI-verb coverage table");
	return [...verbs].sort();
}

function requireCoverage(markdown: string, inventory: readonly { readonly verb: string }[]): void {
	const explicit = new Set(inventory.map((row) => row.verb));
	const missing = skillVerbs(markdown).filter((verb) => !explicit.has(verb));
	if (missing.length > 0) {
		throw new Error(
			`SKILL.md verbs missing explicit rs serve-or-refuse rows: ${missing.join(", ")}`,
		);
	}
}

describe("actual skill CLI table has explicit rs serve-or-refuse coverage", () => {
	it("covers every documented top-level verb using RS_ROUTE_TABLE, not a duplicate verb list", () => {
		expect(() => requireCoverage(skill, RS_ROUTE_TABLE)).not.toThrow();
	});

	it("rejects a synthetic verb added to the actual skill table", () => {
		const synthetic = "synthetic-unported-verb";
		const changed = skill.replace(
			/^(\|\s*CLI verb\s*\|[^\n]*\n\|[-| :]+\|[ \t]*)$/m,
			`$1\n| \`${synthetic}\` | test-only row |`,
		);
		expect(changed).not.toBe(skill);
		expect(skillVerbs(changed)).toContain(synthetic);
		expect(() => requireCoverage(changed, RS_ROUTE_TABLE)).toThrow(synthetic);
	});

	it("strips parenthetical leaf annotations without losing following top-level verbs", () => {
		const example = [
			"## Registry",
			"| `not-a-cli-verb` | skill route |",
			"## CLI-verb coverage (example)",
			"| CLI verb | lives in |",
			"|---|---|",
			"| `family` (`show` / `set` / `--attach [%pane]`) `after` | `not-a-verb` |",
			"| `other` (`list/run/spawn`) | example |",
			"## Global invariants",
			"| `outside-section` | ignored |",
		].join("\n");
		expect(skillVerbs(example)).toEqual(["after", "family", "other"]);
	});

	it("rejects removal of any real skill verb from the explicit inventory", () => {
		for (const verb of skillVerbs(skill)) {
			const remaining = RS_ROUTE_TABLE.filter((row) => row.verb !== verb);
			expect(() => requireCoverage(skill, remaining), verb).toThrow(verb);
		}
	});
});
