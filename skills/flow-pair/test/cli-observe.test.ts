// skills/flow-pair/test/cli-observe.test.ts
// T006: CLI subprocess tests for `flow-pair observe`
// ≥3 tests: success stdout, --json ObserveResult shape, error exit
// Pattern mirrors cli-dispatch.test.ts from Phase 4.

import { execSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { LedgerWriter, nodeLedgerDeps } from "../lib/ledger.js";

const CLI_PATH = join(process.cwd(), "skills/flow-pair/lib/cli.ts");

describe("flow-pair observe — CLI subprocess (T006)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let repoDir: string;
	let runId: string;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "observe-cli-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		repoDir = join(tmpRoot, "repo");

		// Set up real git fixture
		mkdirSync(repoDir, { recursive: true });
		execSync("git init", { cwd: repoDir, encoding: "utf8" });
		execSync("git config user.email ci@test.com", { cwd: repoDir });
		execSync("git config user.name CI", { cwd: repoDir });
		writeFileSync(join(repoDir, "README.md"), "# Test\n");
		execSync("git add README.md", { cwd: repoDir });
		execSync("git commit -m init", { cwd: repoDir });
		writeFileSync(join(repoDir, "src.ts"), "export const x = 1;\n");
		execSync("git add src.ts", { cwd: repoDir });

		// Set up ledger with a real run (createRun scaffolds diffs/ subdir)
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const runResult = writer.createRun("test-repo-obs");
		if (!runResult.ok || !runResult.run) throw new Error("createRun failed in beforeEach");
		runId = runResult.run.runId;
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("success: stdout = exactly 'diffId: diff-NNNN' (one line, exit 0)", () => {
		const proc = spawnSync(
			"npx",
			[
				"tsx",
				CLI_PATH,
				"observe",
				"--run-id",
				runId,
				"--delegation",
				"dlg-0001",
				"--repo",
				repoDir,
				"--ledger-root",
				ledgerRoot,
			],
			{ cwd: process.cwd(), encoding: "utf8", timeout: 30_000 },
		);
		expect(proc.status).toBe(0);
		const nonEmpty = proc.stdout.split("\n").filter((l) => l.trim());
		expect(nonEmpty).toHaveLength(1);
		const line = nonEmpty[0];
		if (!line) throw new Error("stdout was empty");
		expect(line).toMatch(/^diffId: diff-\d{4}$/);
	}, 40_000);

	it("--json: stdout parses as full ObserveResult shape (exit 0)", () => {
		const proc = spawnSync(
			"npx",
			[
				"tsx",
				CLI_PATH,
				"observe",
				"--run-id",
				runId,
				"--delegation",
				"dlg-0001",
				"--repo",
				repoDir,
				"--ledger-root",
				ledgerRoot,
				"--json",
			],
			{ cwd: process.cwd(), encoding: "utf8", timeout: 30_000 },
		);
		expect(proc.status).toBe(0);
		const out = JSON.parse(proc.stdout) as Record<string, unknown>;
		expect(out.ok).toBe(true);
		expect(typeof out.diffId).toBe("string");
		expect(out.diffId as string).toMatch(/^diff-\d{4}$/);
		expect(Array.isArray(out.changedFiles)).toBe(true);
		expect(typeof out.patchPath).toBe("string");
		expect(typeof out.statPath).toBe("string");
		expect(typeof out.manifestPath).toBe("string");
		expect(typeof out.runId).toBe("string");
		expect(typeof out.delegationId).toBe("string");
	}, 40_000);

	it("error: invalid --run-id → stderr + exit 2", () => {
		const proc = spawnSync(
			"npx",
			[
				"tsx",
				CLI_PATH,
				"observe",
				"--run-id",
				"../evil",
				"--delegation",
				"dlg-0001",
				"--repo",
				repoDir,
				"--ledger-root",
				ledgerRoot,
			],
			{ cwd: process.cwd(), encoding: "utf8", timeout: 30_000 },
		);
		expect(proc.status).toBe(2);
		expect(proc.stderr.length).toBeGreaterThan(0);
	}, 40_000);
});
