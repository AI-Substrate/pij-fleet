import { describe, expect, it } from "vitest";
import { type CheckIo, check } from "./docs-bundle-check.js";

const START = "# Start\n\nOne line.\n";
const START_PATH = "docs/cli/agents-start-here.md";
const TYPE_PATH = "templates/types/coder/v1/type.dd.json";
const TYPE = { sections: [{ name: "meta", value: { id: "coder" } }] };

type Compiled = { path: string; text: string } | { path: string; document: unknown };

/** A fake pij-rs bundling `bundled` (page → its compiled sources) over a repo holding `files`. */
function io(
	bundled: Record<string, Compiled[]>,
	files: Record<string, string>,
	extra: Partial<CheckIo> = {},
): CheckIo {
	return {
		run: (args) => {
			if (args[1] === "list") {
				const pages = Object.keys(bundled).map((page) => ({ page }));
				return { code: 0, stdout: JSON.stringify({ ok: true, data: { pages } }) };
			}
			const sources = bundled[args[2] ?? ""];
			if (sources === undefined) return { code: 2, stdout: "refused" };
			return { code: 0, stdout: JSON.stringify({ ok: true, data: { sources } }) };
		},
		readFile: (path) => files[path],
		cliFiles: () => Object.keys(files).filter((path) => path.startsWith("docs/cli/")),
		...extra,
	};
}

const start = { "agents-start-here": [{ path: START_PATH, text: START }] };
const coder = { "types/coder": [{ path: TYPE_PATH, document: TYPE }] };

describe("docs-bundle-check (PRD 009 req-04)", () => {
	it("passes when every compiled source equals its file", () => {
		const report = check(
			io({ ...start, ...coder }, { [START_PATH]: START, [TYPE_PATH]: JSON.stringify(TYPE) }),
		);
		expect(report).toEqual({ ok: true, checked: 2, problems: [] });
	});

	it("fails naming a page edited since the binary was built", () => {
		const report = check(io(start, { [START_PATH]: `${START}edited\n` }));
		expect(report.problems).toEqual([
			`agents-start-here: the bundled page differs from ${START_PATH}; rebuild pij-rs`,
		]);
	});

	it("reads a generated page's document as JSON: layout passes, a changed value fails", () => {
		const relaid = check(io(coder, { [TYPE_PATH]: JSON.stringify(TYPE, null, "\t") }));
		expect(relaid.ok).toBe(true);
		const changed = { sections: [{ name: "meta", value: { id: "coder", line: "new" } }] };
		const stale = check(io(coder, { [TYPE_PATH]: JSON.stringify(changed) }));
		expect(stale.problems).toEqual([
			`types/coder: the bundled page was generated from an older ${TYPE_PATH}; rebuild pij-rs`,
		]);
	});

	it("fails naming a page file that is not bundled", () => {
		const report = check(
			io(start, { [START_PATH]: START, "docs/cli/building-a-role.md": "# B\n" }),
		);
		expect(report.problems).toEqual([
			"docs/cli/building-a-role.md is not bundled: add it to pages() in crates/cli/src/docs.rs",
		]);
	});

	it("fails naming a source whose file is gone", () => {
		const report = check(io(coder, {}));
		expect(report.problems).toEqual([`types/coder: its source ${TYPE_PATH} is missing`]);
	});

	it("refuses a binary with no docs verb, no pages or no sources", () => {
		const none = check(io({}, {}, { run: () => ({ code: 2, stdout: "unrecognized subcommand" }) }));
		expect(none.problems[0]).toContain("pij-rs docs list --json exited 2");
		expect(check(io({}, {})).problems).toEqual([
			"pij-rs bundles no pages; refusing to pass on nothing",
		]);
		expect(check(io({ bare: [] }, {})).problems).toEqual(["bare: pij-rs names no source for it"]);
	});
});
