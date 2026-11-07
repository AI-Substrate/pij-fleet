import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
	APPROVED_EXEMPTIONS,
	coverageFindings,
	type Doc,
	EXEMPTIONS,
	type Exemption,
	type Reader,
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
const NONE = { rows: [] };

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
			NONE,
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
		expect(
			coverageFindings([], [], [umbrella("complete"), child("in-progress")], NONE, []),
		).toEqual([`${W1}#steps/s01: complete while its child ${W2}#steps/s01 is in-progress`]);
		expect(coverageFindings([], [], [umbrella("complete"), child("complete")], NONE, [])).toEqual(
			[],
		);
		expect(coverageFindings([], [], [umbrella("complete"), child("abandoned")], NONE, [])).toEqual(
			[],
		);
		expect(coverageFindings([], [], [umbrella("in-progress"), child("planned")], NONE, [])).toEqual(
			[],
		);
		expect(
			coverageFindings(
				[],
				[],
				[steps(W2, { id: "s01", state: "planned", part_of: "#steps/s09" })],
				NONE,
				[],
			),
		).toEqual([`${W2}#steps/s01: part_of ${W2}#steps/s09, which is no step`]);
	});

	describe("exemptions only shrink", () => {
		const exempt = (id: string, owner = "docs-system", why = "test"): Exemption => ({
			address: `${PRD}#requirements/${id}`,
			owner,
			why,
		});
		const baseline = [`${PRD}#requirements/req-02`, `${PRD}#requirements/req-03`];
		const delivering = (...ids: string[]) =>
			steps(W1, {
				id: "s01",
				state: "planned",
				delivers: ids.map((id) => `../prds/001-x.dd.json#requirements/${id}`),
			});
		const check = (
			requirements: string[],
			delivered: string[],
			rows: Exemption[],
			approved = baseline,
		) =>
			coverageFindings(
				[prd(...requirements.map((id) => ({ id })))],
				[],
				[delivering(...delivered)],
				{ rows },
				approved,
			);

		it("an approved exemption hides only its own gap", () => {
			expect(
				check(["req-01", "req-02", "req-03"], ["req-01"], [exempt("req-02"), exempt("req-03")]),
			).toEqual([]);
		});

		it("a covered address left in the baseline fails; removed from both it passes", () => {
			const requirements = ["req-01", "req-02", "req-03"];
			const delivered = ["req-01", "req-02"];
			expect(check(requirements, delivered, [exempt("req-03")])).toEqual([
				`${PRD}#requirements/req-02: covered; remove it from the baseline (APPROVED_EXEMPTIONS in coverage-check.ts)`,
			]);
			expect(
				check(requirements, delivered, [exempt("req-03")], [`${PRD}#requirements/req-03`]),
			).toEqual([]);
		});

		it("a deleted row left in the baseline fails; removed from the PRD, JSON and baseline it passes", () => {
			expect(check(["req-01", "req-02"], ["req-01"], [exempt("req-02")])).toEqual([
				`${PRD}#requirements/req-03: row missing; remove it from the baseline (APPROVED_EXEMPTIONS in coverage-check.ts)`,
			]);
			expect(
				check(["req-01", "req-02"], ["req-01"], [exempt("req-02")], [`${PRD}#requirements/req-02`]),
			).toEqual([]);
		});

		it("a baseline and JSON that list different addresses fail", () => {
			expect(check(["req-01", "req-02", "req-03"], ["req-01"], [exempt("req-02")])).toEqual([
				`${PRD}#requirements/req-03: no workstream step delivers this requirement (deliver it, or set it dropped)`,
				`${PRD}#requirements/req-03: in the baseline (APPROVED_EXEMPTIONS in coverage-check.ts) but has no valid row in ${EXEMPTIONS}; the two must list the same addresses`,
			]);
		});

		it("a new real planned requirement with an exemption for it fails", () => {
			expect(
				check(
					["req-01", "req-02", "req-03", "req-04"],
					["req-01"],
					[exempt("req-02"), exempt("req-03"), exempt("req-04")],
				),
			).toEqual([
				`${PRD}#requirements/req-04: exemption not in the approved baseline (APPROVED_EXEMPTIONS in coverage-check.ts); deliver it or set it dropped`,
				`${PRD}#requirements/req-04: no workstream step delivers this requirement (deliver it, or set it dropped)`,
			]);
		});

		it("swapping an approved address for another one fails, at the same count", () => {
			const findings = check(
				["req-01", "req-02", "req-03", "req-04"],
				["req-01", "req-03"],
				[exempt("req-02"), exempt("req-04")],
			);
			expect(findings).toContain(
				`${PRD}#requirements/req-04: exemption not in the approved baseline (APPROVED_EXEMPTIONS in coverage-check.ts); deliver it or set it dropped`,
			);
			expect(findings).toContain(
				`${PRD}#requirements/req-04: no workstream step delivers this requirement (deliver it, or set it dropped)`,
			);
		});

		it("a duplicate, an empty owner or an empty why fails", () => {
			const rows = ["req-01", "req-02", "req-03"];
			expect(
				check(rows, ["req-01"], [exempt("req-02"), exempt("req-02"), exempt("req-03")]),
			).toEqual([`${PRD}#requirements/req-02: exempted twice`]);
			for (const bad of [exempt("req-03", " "), exempt("req-03", "docs-system", "")])
				expect(check(rows, ["req-01"], [exempt("req-02"), bad])).toEqual([
					`${PRD}#requirements/req-03: exemption needs a non-empty address, owner and why`,
					`${PRD}#requirements/req-03: no workstream step delivers this requirement (deliver it, or set it dropped)`,
					`${PRD}#requirements/req-03: in the baseline (APPROVED_EXEMPTIONS in coverage-check.ts) but has no valid row in ${EXEMPTIONS}; the two must list the same addresses`,
				]);
		});

		it("a stale or unknown exemption fails", () => {
			const approved = [...baseline, `${PRD}#requirements/req-99`];
			expect(
				check(
					["req-01", "req-02", "req-03"],
					["req-01", "req-02"],
					[exempt("req-02"), exempt("req-03")],
				),
			).toEqual([
				`${PRD}#requirements/req-02: exempted, but now covered; remove the exemption here and in APPROVED_EXEMPTIONS (owner docs-system)`,
			]);
			expect(
				check(
					["req-01", "req-02", "req-03"],
					["req-01"],
					[exempt("req-02"), exempt("req-03"), exempt("req-99")],
					approved,
				),
			).toEqual([
				`${PRD}#requirements/req-99: exempted, but no such row; remove the exemption here and in APPROVED_EXEMPTIONS`,
			]);
		});

		it("a malformed exemptions file fails", () => {
			expect(coverageFindings([], [], [], { entries: [] }, [])).toEqual([
				`${EXEMPTIONS}: rows is not an array`,
			]);
		});
	});

	describe("inputs fail closed", () => {
		it("a missing or non-array required section fails", () => {
			expect(coverageFindings([doc(PRD, { requirements: "req-01" })], [], [], NONE, [])).toEqual([
				`${PRD}: required section requirements is not an array`,
			]);
			expect(coverageFindings([], [doc(BLUEPRINT, {})], [], NONE, [])).toEqual([
				`${BLUEPRINT}: required section components is missing`,
			]);
			expect(coverageFindings([], [], [{ path: W1, body: {} }], NONE, [])).toEqual([
				`${W1}: has no sections array`,
			]);
		});

		const healthy: Record<string, Record<string, unknown>> = {
			"docs/project/prds": {
				"001-x.dd.json": { requirements: [{ id: "req-01", state: "planned" }] },
			},
			"docs/project/blueprints": {
				"001-x.dd.json": { components: [{ id: "c-a", state: "planned" }] },
			},
			"docs/project/workstreams": {
				"001-x.dd.json": {
					steps: [
						{
							id: "s01",
							state: "planned",
							delivers: ["../prds/001-x.dd.json#requirements/req-01"],
							builds: ["../blueprints/001-x.dd.json#components/c-a"],
						},
					],
				},
			},
		};
		const errno = (code: string) => Object.assign(new Error(code), { code });
		/** A fake repo at /r: `broken` maps a folder to the error listing it throws. */
		const reader = (broken: Record<string, string> = {}, files = healthy): Reader => ({
			readdir: (path) => {
				const folder = path.slice("/r/".length);
				if (broken[folder]) throw errno(broken[folder]);
				if (!files[folder]) throw errno("ENOENT");
				return Object.keys(files[folder]);
			},
			readFile: (path) => {
				const relative = path.slice("/r/".length);
				if (relative === EXEMPTIONS) return JSON.stringify(NONE);
				const folder = relative.slice(0, relative.lastIndexOf("/"));
				const sections = files[folder]?.[relative.slice(folder.length + 1)];
				if (sections === undefined) throw errno("ENOENT");
				if (typeof sections === "string") return sections;
				return JSON.stringify({
					sections: Object.entries(sections as object).map(([name, value]) => ({ name, value })),
				});
			},
		});

		it("a healthy repo with no legacy roadmaps folder passes", () => {
			expect(repoFindings("/r", reader(), []).findings).toEqual([]);
		});

		it("a required folder that can't be listed fails, even when absent; only the legacy alias may be absent", () => {
			for (const code of ["EACCES", "EIO", "ENOENT"])
				expect(
					repoFindings("/r", reader({ "docs/project/blueprints": code }), []).findings,
				).toEqual([`docs/project/blueprints: cannot read this folder (${code})`]);
			expect(
				repoFindings("/r", reader({ "docs/project/roadmaps": "EACCES" }), []).findings,
			).toEqual(["docs/project/roadmaps: cannot read this folder (EACCES)"]);
		});

		it("a required folder with no documents, or a document that won't parse, fails", () => {
			expect(
				repoFindings("/r", reader({}, { ...healthy, "docs/project/blueprints": {} }), []).findings,
			).toEqual(["docs/project/blueprints: required folder has no .dd.json documents"]);
			const broken = { ...healthy, "docs/project/prds": { "001-x.dd.json": "{ not json" } };
			expect(repoFindings("/r", reader({}, broken), []).findings).toEqual([
				expect.stringMatching(/^docs\/project\/prds\/001-x\.dd\.json: cannot read or parse/),
			]);
		});
	});

	it("this repo is covered, with only its owned exemptions", () => {
		const { findings, exempted } = repoFindings(resolve(import.meta.dirname, "../.."));
		expect(findings).toEqual([]);
		expect(exempted).toBeGreaterThan(0);
		expect(exempted).toBeLessThanOrEqual(APPROVED_EXEMPTIONS.length);
	});
});
