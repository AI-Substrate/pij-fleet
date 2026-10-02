import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { scanRepository, scanText, shouldScanFile } from "./local-path-check.js";

describe("local-path-check", () => {
	it("scans operational surfaces and excludes tests, fixtures, and snapshots", () => {
		expect(shouldScanFile("justfile")).toBe(true);
		expect(shouldScanFile(".omp/extensions/pij/cli.ts")).toBe(true);
		expect(shouldScanFile(".omp/extensions/pij/cli.ts")).toBe(true);
		expect(shouldScanFile("harness/scripts/tool.sh")).toBe(true);
		expect(shouldScanFile("skills/pij/SKILL.md")).toBe(true);
		expect(shouldScanFile(".omp/extensions/pij/cli.test.ts")).toBe(false);
		expect(shouldScanFile(".omp/extensions/pij/cli.test.ts")).toBe(false);
		expect(shouldScanFile("harness/fixtures/output.json")).toBe(false);
		expect(shouldScanFile("agents/a/__snapshots__/run.json")).toBe(false);
		expect(shouldScanFile("agents/a/.minih-source.json")).toBe(false);
		expect(shouldScanFile("docs/how/pij.md")).toBe(false);
	});

	it("detects macOS, Linux, WSL, and Windows user homes", () => {
		const findings = scanText(
			"justfile",
			[
				'open("/Users/alice/.pi/agent/settings.json")',
				'open("/home/bob/.config/tool.json")',
				'open("/mnt/c/Users/Casey/AppData/tool.json")',
				String.raw`open("C:\Users\Dana\AppData\tool.json")`,
			].join("\n"),
		);

		expect(findings.map((finding) => finding.kind)).toEqual([
			"macOS user home",
			"Linux user home",
			"WSL Windows user home",
			"Windows user home",
		]);
	});

	it("allows portable paths, comments, and explicitly marked fixtures", () => {
		const findings = scanText(
			"harness/scripts/check.sh",
			[
				'open("$HOME/.pi/agent/settings.json")',
				'open("~/.pi/agent/settings.json")',
				'open("./fixtures/settings.json")',
				"# /Users/alice/example-only",
				"# local-path-check: allow -- forbidden-path regression fixture",
				"grep '/Users/alice/old-repo' fixture",
				"grep '/home/bob/old-repo' fixture # local-path-check: allow",
			].join("\n"),
		);

		expect(findings).toEqual([]);
	});

	it("scans both tracked and untracked non-ignored operational files", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-local-path-check-"));
		try {
			execFileSync("git", ["init", "--quiet"], { cwd: root });
			writeFileSync(join(root, "justfile"), 'open("/Users/tracked/.config/tool.json")\n');
			mkdirSync(join(root, "harness", "scripts"), { recursive: true });
			writeFileSync(
				join(root, "harness", "scripts", "new-tool.ts"),
				'const config = "/home/untracked/.config/tool.json";\n',
			);
			execFileSync("git", ["add", "justfile"], { cwd: root });

			expect(
				scanRepository(root)
					.map((finding) => finding.file)
					.sort(),
			).toEqual(["harness/scripts/new-tool.ts", "justfile"]);
		} finally {
			rmSync(root, { recursive: true, force: true, maxRetries: 3 });
		}
	});
});
