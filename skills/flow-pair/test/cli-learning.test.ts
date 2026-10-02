// skills/flow-pair/test/cli-learning.test.ts
// Phase 7: CLI subprocess tests for 'learn' subcommand.

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { PROMPT_CLUSTERS, type PromptCluster } from "../lib/learning.js";

const CLI_PATH = join(import.meta.dirname, "..", "lib", "cli.ts");
const RUN_ID = "2026-06-23T00-00-00Z-cli-learning-t007";

function runCli(args: string[]): { stdout: string; stderr: string; status: number } {
	const r = spawnSync("npx", ["tsx", CLI_PATH, ...args], {
		encoding: "utf8",
		timeout: 15000,
	});
	return {
		stdout: r.stdout ?? "",
		stderr: r.stderr ?? "",
		status: r.status ?? 1,
	};
}

function scaffoldRunDir(ledgerRoot: string, runId: string): void {
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
}

function makePromptLabFixture(promptLabRoot: string): void {
	for (const cluster of PROMPT_CLUSTERS) {
		const clusterDir = join(promptLabRoot, "clusters", cluster);
		mkdirSync(join(clusterDir, "candidates"), { recursive: true });
		writeFileSync(join(clusterDir, "active.md"), `# ${cluster} active\n`);
		writeFileSync(join(clusterDir, "candidates", ".gitkeep"), "");
		writeFileSync(join(clusterDir, "changelog.md"), `# ${cluster} changelog\n`);
	}
}

function candidateMdFiles(promptLabRoot: string, cluster: PromptCluster): string[] {
	return readdirSync(join(promptLabRoot, "clusters", cluster, "candidates"))
		.filter((name) => name.endsWith(".md"))
		.sort();
}

describe("flow-pair learn — CLI subprocess (T007)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let promptLabRoot: string;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "cli-learning-"));
		ledgerRoot = join(tmpRoot, "ledger");
		promptLabRoot = join(tmpRoot, "prompt-lab");
		scaffoldRunDir(ledgerRoot, RUN_ID);
		makePromptLabFixture(promptLabRoot);
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("learn: stdout = exactly 'learning: learn-0001' and writes implement-code candidate", () => {
		const r = runCli([
			"learn",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--cluster",
			"implement-code",
			"--miss-type",
			"implement-code",
			"--summary",
			"missed a subprocess assertion",
			"--candidate-delta",
			"Add stdout-contract tests for CLI changes.",
			"--evidence",
			"review finding;mutation failure",
			"--ledger-root",
			ledgerRoot,
			"--prompt-lab-root",
			promptLabRoot,
		]);

		expect(r.status).toBe(0);
		expect(r.stdout.trim()).toBe("learning: learn-0001");
		const cliStderr = r.stderr
			.split("\n")
			.filter((line) => !line.startsWith("npm warn "))
			.join("")
			.trim();
		expect(cliStderr).toBe("");
		expect(candidateMdFiles(promptLabRoot, "implement-code")).toEqual(["learn-0001.md"]);
		expect(candidateMdFiles(promptLabRoot, "fix-code")).toEqual([]);
	});

	it("learn --json: stdout includes candidatePath and cluster", () => {
		const r = runCli([
			"learn",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--cluster",
			"implement-code",
			"--miss-type",
			"implement-code",
			"--summary",
			"record prompt learning",
			"--candidate-delta",
			"Keep candidate notes isolated by prompt cluster.",
			"--ledger-root",
			ledgerRoot,
			"--prompt-lab-root",
			promptLabRoot,
			"--json",
		]);

		expect(r.status).toBe(0);
		const parsed = JSON.parse(r.stdout) as {
			ok: boolean;
			candidatePath: string;
			cluster: string;
			candidate: { learningId: string; candidatePath: string; cluster: string };
		};
		expect(parsed.ok).toBe(true);
		expect(parsed.cluster).toBe("implement-code");
		expect(parsed.candidate.cluster).toBe("implement-code");
		expect(parsed.candidate.learningId).toBe("learn-0001");
		expect(parsed.candidatePath).toBe(
			join(promptLabRoot, "clusters", "implement-code", "candidates", "learn-0001.md"),
		);
		expect(readFileSync(parsed.candidate.candidatePath, "utf8")).toContain(
			"No automatic promotion",
		);
	});

	it("learn invalid cluster exits 2 and writes no candidate", () => {
		const r = runCli([
			"learn",
			"--run-id",
			RUN_ID,
			"--delegation-id",
			"dlg-0001",
			"--cluster",
			"../fix-code",
			"--miss-type",
			"implement-code",
			"--summary",
			"invalid cluster",
			"--ledger-root",
			ledgerRoot,
			"--prompt-lab-root",
			promptLabRoot,
		]);

		expect(r.status).toBe(2);
		expect(r.stderr).toMatch(/cluster/);
		expect(candidateMdFiles(promptLabRoot, "implement-code")).toEqual([]);
		expect(candidateMdFiles(promptLabRoot, "fix-code")).toEqual([]);
	});
});
