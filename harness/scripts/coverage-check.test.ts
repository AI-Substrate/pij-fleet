import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
	coverageFindings,
	type Doc,
	type Exemption,
	repoFindings,
	resolveLink,
} from "./coverage-check.js";

const PRD = "docs/project/prds/001-x.dd.json";
const BLUEPRINT = "docs/project/blueprints/001-x.dd.json";
const W1 = "docs/project/workstreams/001-x.dd.json";
const W2 = "docs/project/workstreams/002-y.dd.json";

const doc = (path: string, sections: Record<string, unknown>): Doc => ({
	path,
	body: { sections: Object.entries(sections).map(([name, value]) => ({ name, value })) },
});

const prd = (...rows: { id: string; state?: string }[]) =>
	doc(PRD, { requirements: rows.map((row) => ({ state: "planned", ...row })) });
const blueprint = (...ids: string[]) =>
	doc(BLUEPRINT, { components: ids.map((id) => ({ id, state: "planned" })) });
const steps = (path: string, ...rows: Record<string, unknown>[]) => doc(path, { steps: rows });

describe("coverage-check (plan 210, req-31)", () => {
	it("resolves link cells against the document's own folder", () => {
		expect(resolveLink(W1, "../prds/001-x.dd.json#requirements/req-01")).toBe(
			`${PRD}#requirements/req-01`,
		);
		expect(resolveLink(W1, "#steps/s02")).toBe(`${W1}#steps/s02`);
		expect(resolveLink(W2, "001-x.dd.json#steps/s01")).toBe(`${W1}#steps/s01`);
	});

	it("names a requirement no step delivers and a component no step builds, but not a dropped one", () => {
		const findings = coverageFindings(
			[prd({ id: "req-01" }, { id: "req-02" }, { id: "req-03", state: "dropped" })],
			[blueprint("c-a", "c-b")],
			[
				steps(W1, {
					id: "s01",
					state: "planned",
					delivers: ["../prds/001-x.dd.json#requirements/req-01"],
					builds: ["../blueprints/001-x.dd.json#components/c-a"],
				}),
			],
			[],
		);
		expect(findings).toHaveLength(2);
		expect(findings[0]).toMatch(
			/^docs\/project\/prds\/001-x\.dd\.json#requirements\/req-02: no workstream step delivers/,
		);
		expect(findings[1]).toMatch(/#components\/c-b: no workstream step builds/);
	});

	it("an umbrella is complete only when every child is complete or abandoned, across files", () => {
		const umbrella = (state: string) => steps(W1, { id: "s01", state });
		const child = (state: string) =>
			steps(W2, { id: "s01", state, part_of: "001-x.dd.json#steps/s01" });
		expect(coverageFindings([], [], [umbrella("complete"), child("in-progress")], [])).toEqual([
			`${W1}#steps/s01: complete while its child ${W2}#steps/s01 is in-progress`,
		]);
		expect(coverageFindings([], [], [umbrella("complete"), child("complete")], [])).toEqual([]);
		expect(coverageFindings([], [], [umbrella("complete"), child("abandoned")], [])).toEqual([]);
		expect(coverageFindings([], [], [umbrella("in-progress"), child("planned")], [])).toEqual([]);
		expect(
			coverageFindings(
				[],
				[],
				[steps(W2, { id: "s01", state: "planned", part_of: "#steps/s09" })],
				[],
			),
		).toEqual([`${W2}#steps/s01: part_of ${W2}#steps/s09, which is no step`]);
	});

	it("an exemption hides only its own gap, and a stale or unknown exemption fails", () => {
		const exempt = (id: string): Exemption => ({
			address: `${PRD}#requirements/${id}`,
			owner: "docs-system",
			why: "test",
		});
		const delivering = steps(W1, {
			id: "s01",
			state: "planned",
			delivers: ["../prds/001-x.dd.json#requirements/req-01"],
		});
		const rows = prd({ id: "req-01" }, { id: "req-02" });
		expect(coverageFindings([rows], [], [delivering], [exempt("req-02")])).toEqual([]);
		expect(
			coverageFindings([rows], [], [delivering], [exempt("req-02"), exempt("req-01")]),
		).toEqual([
			`${PRD}#requirements/req-01: exempted, but now covered; remove the exemption (owner docs-system)`,
		]);
		expect(
			coverageFindings([rows], [], [delivering], [exempt("req-02"), exempt("req-99")]),
		).toEqual([`${PRD}#requirements/req-99: exempted, but no such row; remove the exemption`]);
	});

	it("this repo is covered, with only its owned exemptions", () => {
		const { findings, exempted } = repoFindings(resolve(import.meta.dirname, "../.."));
		expect(findings).toEqual([]);
		expect(exempted).toBeGreaterThan(0);
	});
});
