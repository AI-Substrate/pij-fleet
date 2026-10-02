// skills/flow-pair/test/cli-review.test.ts
// Phase 6: CLI subprocess tests for 'review' and 'fix' subcommands.

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

// ─── Fixture helpers ──────────────────────────────────────────────────────────

function scaffoldRunDir(ledgerRoot: string, runId: string): string {
	const runDir = join(ledgerRoot, "runs", runId);
	for (const sub of [
		"",
		"reviews",
		"fix-packets",
		"delegations",
		"prompt-trials",
		"learnings",
		"prompts",
		"worker-reports",
		"diffs",
	]) {
		mkdirSync(join(runDir, sub), { recursive: true });
	}
	writeFileSync(
		join(runDir, "run.json"),
		JSON.stringify({ runId, repoId: "test", status: "open", createdAt: new Date().toISOString() }),
	);
	writeFileSync(join(runDir, "events.jsonl"), "");
	return runDir;
}

/** Minimal worker-fix.md template for fix subcommand tests. */
const MINIMAL_TEMPLATE =
	"# Fix {{FIX_PACKET_ID}}\nDelegation: {{DELEGATION_ID}}\nReview: {{REVIEW_ID}}\nAllowed:\n{{ALLOWED_FILES_LIST}}\nFindings:\n{{FINDINGS_SUMMARY}}\n";

const CLI_PATH = join(import.meta.dirname, "..", "lib", "cli.ts");

function runCli(
	args: string[],
	opts?: { cwd?: string },
): { stdout: string; stderr: string; status: number } {
	const r = spawnSync("npx", ["tsx", CLI_PATH, ...args], {
		encoding: "utf8",
		timeout: 15000,
		cwd: opts?.cwd ?? process.cwd(),
	});
	return {
		stdout: r.stdout ?? "",
		stderr: r.stderr ?? "",
		status: r.status ?? 1,
	};
}

// ─── CLI subprocess tests ─────────────────────────────────────────────────────

describe("flow-pair review + fix — CLI subprocess (T007)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let repoRoot: string;
	let phaseDir: string;
	let templateDir: string;
	let runDir: string;
	const RUN_ID = "2026-06-23T00-00-00Z-cli-review-t007";

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "cli-review-"));
		ledgerRoot = join(tmpRoot, "ledger");
		repoRoot = join(tmpRoot, "repo");
		phaseDir = join(repoRoot, "docs", "plans", "phase-6");
		templateDir = join(tmpRoot, "templates");
		runDir = scaffoldRunDir(ledgerRoot, RUN_ID);
		mkdirSync(phaseDir, { recursive: true });
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-fix.md"), MINIMAL_TEMPLATE);
		// No execution.log.md → FIX_REQUIRED
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("review: stdout = exactly 'verdict: FIX_REQUIRED' (missing log), exit 0", () => {
		const r = runCli([
			"review",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--phase-dir",
			phaseDir,
			"--ledger-root",
			ledgerRoot,
			"--repo-root",
			repoRoot,
		]);
		expect(r.status).toBe(0);
		expect(r.stdout.trim()).toBe("verdict: FIX_REQUIRED");
	});

	it("review --json: stdout parses as JSON with verdict field and findings array", () => {
		const r = runCli([
			"review",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--phase-dir",
			phaseDir,
			"--ledger-root",
			ledgerRoot,
			"--repo-root",
			repoRoot,
			"--json",
		]);
		expect(r.status).toBe(0);
		const parsed = JSON.parse(r.stdout) as Record<string, unknown>;
		expect(parsed.verdict).toBe("FIX_REQUIRED");
		expect(Array.isArray(parsed.findings)).toBe(true);
	});

	it("review: missing required arg → non-zero exit + stderr message", () => {
		const r = runCli(["review", "--run-id", RUN_ID]);
		expect(r.status).not.toBe(0);
		expect(r.stderr.length).toBeGreaterThan(0);
	});

	it("fix --json: fix packet has allowedFiles array and pointerMsg", () => {
		// First run review to get a review record with a finding
		runCli([
			"review",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--phase-dir",
			phaseDir,
			"--ledger-root",
			ledgerRoot,
			"--repo-root",
			repoRoot,
		]);
		const r = runCli([
			"fix",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--review-id",
			"rev-0001",
			"--ledger-root",
			ledgerRoot,
			"--repo-root",
			repoRoot,
			"--template-dir",
			templateDir,
			"--json",
		]);
		expect(r.status).toBe(0);
		const parsed = JSON.parse(r.stdout) as Record<string, unknown>;
		expect(Array.isArray(parsed.allowedFiles)).toBe(true);
		expect(typeof parsed.pointerMsg).toBe("string");
	});

	it("fix rejects traversal runId before reading escaped review records", () => {
		mkdirSync(join(ledgerRoot, "evil", "reviews"), { recursive: true });
		writeFileSync(join(ledgerRoot, "evil", "reviews", "rev-0001.json"), "SENTINEL_RUN_TRAVERSAL");

		const r = runCli([
			"fix",
			"--run-id",
			"../evil",
			"--delegation-id",
			"dlg-0001",
			"--review-id",
			"rev-0001",
			"--ledger-root",
			ledgerRoot,
			"--repo-root",
			repoRoot,
			"--template-dir",
			templateDir,
		]);

		expect(r.status).not.toBe(0);
		expect(r.stderr.length).toBeGreaterThan(0);
		expect(`${r.stdout}\n${r.stderr}`).not.toMatch(/SENTINEL|not valid JSON|Unexpected token/);
	});

	it("fix rejects traversal reviewId before reading outside reviews/", () => {
		writeFileSync(join(runDir, "evil.json"), "SENTINEL_REVIEW_TRAVERSAL");

		const r = runCli([
			"fix",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--review-id",
			"../evil",
			"--ledger-root",
			ledgerRoot,
			"--repo-root",
			repoRoot,
			"--template-dir",
			templateDir,
		]);

		expect(r.status).not.toBe(0);
		expect(r.stderr.length).toBeGreaterThan(0);
		expect(`${r.stdout}\n${r.stderr}`).not.toMatch(/SENTINEL|not valid JSON|Unexpected token/);
	});
});
