import { describe, expect, it } from "vitest";
import { type CheckIo, check } from "./docs-bundle-check.js";

const START = "# Start\n\nOne line.\n";

/** A fake pij-rs bundling `bundled` (id → [source, text]) over a repo holding `files`. */
function io(
	bundled: Record<string, [string, string]>,
	files: Record<string, string>,
	extra: Partial<CheckIo> = {},
): CheckIo {
	return {
		run: (args) => {
			if (args[1] === "list") {
				const pages = Object.entries(bundled).map(([page, [source]]) => ({ page, source }));
				return { code: 0, stdout: JSON.stringify({ ok: true, data: { pages } }) };
			}
			const page = bundled[args[2] ?? ""];
			if (page === undefined) return { code: 2, stdout: "refused" };
			return { code: 0, stdout: JSON.stringify({ ok: true, data: { text: page[1] } }) };
		},
		readFile: (path) => files[path],
		cliFiles: () => Object.keys(files).filter((path) => path.startsWith("docs/cli/")),
		...extra,
	};
}

describe("docs-bundle-check (PRD 009 req-04)", () => {
	const source = "docs/cli/agents-start-here.md";

	it("passes when every bundled page equals its file", () => {
		const report = check(io({ "agents-start-here": [source, START] }, { [source]: START }));
		expect(report).toEqual({ ok: true, checked: 1, problems: [] });
	});

	it("fails naming a page edited since the binary was built", () => {
		const report = check(
			io({ "agents-start-here": [source, START] }, { [source]: `${START}edited\n` }),
		);
		expect(report.ok).toBe(false);
		expect(report.problems).toEqual([
			`agents-start-here: the bundled page differs from ${source}; rebuild pij-rs`,
		]);
	});

	it("fails naming a page file that is not bundled", () => {
		const report = check(
			io(
				{ "agents-start-here": [source, START] },
				{ [source]: START, "docs/cli/concepts.md": "# Concepts\n" },
			),
		);
		expect(report.problems).toEqual([
			"docs/cli/concepts.md is not bundled: add it to PAGES in crates/cli/src/docs.rs",
		]);
	});

	it("fails naming a bundled page whose file is gone", () => {
		const report = check(io({ "agents-start-here": [source, START] }, {}));
		expect(report.problems).toEqual([`agents-start-here: its source ${source} is missing`]);
	});

	it("covers pages whose source is outside docs/cli", () => {
		const skill = "skills/pij/references/community/concepts.md";
		const ok = check(io({ concepts: [skill, "# C\n"] }, { [skill]: "# C\n" }));
		expect(ok.ok).toBe(true);
		const stale = check(io({ concepts: [skill, "# C\n"] }, { [skill]: "# C2\n" }));
		expect(stale.problems[0]).toContain(`concepts: the bundled page differs from ${skill}`);
	});

	it("refuses a binary with no docs verb or no pages", () => {
		const none = check(io({}, {}, { run: () => ({ code: 2, stdout: "unrecognized subcommand" }) }));
		expect(none.ok).toBe(false);
		expect(none.problems[0]).toContain("pij-rs docs list --json exited 2");
		const empty = check(io({}, {}));
		expect(empty.problems).toEqual(["pij-rs bundles no pages; refusing to pass on nothing"]);
	});
});
