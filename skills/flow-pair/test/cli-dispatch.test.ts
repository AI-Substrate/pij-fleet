// skills/flow-pair/test/cli-dispatch.test.ts
// Fix 1: dispatch stdout is EXACTLY the pointer line (subprocess integration test)
// Fix 2: packet.written event shape matches schema (additionalProperties:false)

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ContextPackManifest } from "../lib/context-pack.js";
import { LedgerWriter, nodeLedgerDeps, PROMPTS_DIR } from "../lib/ledger.js";
import { nodePacketRendererDeps, PacketRenderer } from "../lib/packet.js";

// ─── Shared fixture ───────────────────────────────────────────────────────────

const FIXED_RUN_ID = "2026-06-17T00-00-00Z-p4cli-xxxxx";

const SAMPLE_TEMPLATE = `\
# Worker Packet — {{DELEGATION_ID}}
**Run**: {{RUN_ID}}
## Forbidden Paths
{{FORBIDDEN_PATHS}}
## Allowed Scope
{{ALLOWED_PATHS}}
## Phase Plan
{{PLAN_PHASE_CONTENT}}
## Tasks
{{TASKS_CONTENT}}
## Execution Log
{{EXEC_LOG_CONTENT}}
## Learnings
{{LEARNINGS_CONTENT}}
## Report Schema
\`\`\`json
{"delegationId":"{{DELEGATION_ID}}","outcome":"COMPLETE"}
\`\`\`
## Stop Conditions
- Do NOT edit forbidden paths
`;

function scaffoldRun(ledgerRoot: string, runId: string): string {
	const runDir = join(ledgerRoot, "runs", runId);
	mkdirSync(join(runDir, PROMPTS_DIR), { recursive: true });
	mkdirSync(join(runDir, "prompt-trials"), { recursive: true });
	mkdirSync(join(runDir, "delegations"), { recursive: true });
	return runDir;
}

function makeManifest(
	runId: string,
	overrides: Partial<ContextPackManifest> = {},
): ContextPackManifest {
	return {
		packId: "cp-0001",
		runId,
		delegationId: "dlg-0001",
		phase: "Phase 4: CLI Test",
		cluster: "implement-code",
		entries: [
			{
				path: "/plan.md",
				section: "Phase 4",
				content: "Plan content.",
				hash: "aa",
				role: "plan-phase",
			},
			{ path: "/tasks.md", content: "Tasks.", hash: "bb", role: "tasks" },
		],
		exclusions: [],
		allowedPaths: ["skills/flow-pair/"],
		forbiddenPaths: [".the-flow-state.json", "the-flow.json"],
		createdAt: new Date().toISOString(),
		...overrides,
	};
}

let tmpRoot: string;
let ledgerRoot: string;
let templateDir: string;

beforeEach(() => {
	tmpRoot = mkdtempSync(join(tmpdir(), "pkt-cli-"));
	ledgerRoot = join(tmpRoot, ".flow-pair");
	templateDir = join(tmpRoot, "templates");
	mkdirSync(templateDir, { recursive: true });
	writeFileSync(join(templateDir, "worker-implement.md"), SAMPLE_TEMPLATE, "utf8");
});

afterEach(() => {
	rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
});

// Fix 2: packet.written schema shape ───────────────────────────────────────────

describe("LedgerEvent — packet.written schema shape (Fix 2)", () => {
	it("packet.written event has exactly the schema fields (additionalProperties:false)", () => {
		// Run a real writePacket and verify the emitted event shape matches the schema.
		// Schema requires: type, runId, delegationId, packetPath, at — nothing more.
		const runId = FIXED_RUN_ID;
		const runDir = scaffoldRun(ledgerRoot, runId);
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());

		const result = renderer.writePacket({
			manifest: makeManifest(runId),
			taskDescription: "Test dispatch",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);

		const eventsPath = join(runDir, "events.jsonl");
		expect(existsSync(eventsPath)).toBe(true);

		const lines = readFileSync(eventsPath, "utf8")
			.split("\n")
			.filter((l) => l.trim());
		const events = lines.map((l) => JSON.parse(l) as Record<string, unknown>);

		const pktEvent = events.find((e) => e.type === "packet.written");
		expect(pktEvent).toBeDefined();
		if (!pktEvent) throw new Error("pktEvent should be defined");

		// Verify exact field set (additionalProperties:false — no extras, no missing)
		const expectedKeys = ["type", "runId", "delegationId", "packetPath", "at"].sort();
		expect(Object.keys(pktEvent).sort()).toEqual(expectedKeys);

		// Verify field types
		expect(pktEvent.type).toBe("packet.written");
		expect(typeof pktEvent.runId).toBe("string");
		expect(typeof pktEvent.delegationId).toBe("string");
		expect(typeof pktEvent.packetPath).toBe("string");
		expect(typeof pktEvent.at).toBe("string");
	});
});

// Fix 1: dispatch stdout format ────────────────────────────────────────────────

describe("flow-pair dispatch — stdout format (Fix 1)", () => {
	it("stdout is EXACTLY the pointer line — one line, no prefix, no metadata", () => {
		// Step 1: create a real run via LedgerWriter
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const runResult = writer.createRun("cli-test-repo-xyz");
		expect(runResult.ok).toBe(true);
		const runId = runResult.run?.runId;
		if (runId === undefined) throw new Error("expected createRun to return a run id");

		// Step 2: create plan + tasks fixture files the CLI can read
		const planPath = join(tmpRoot, "plan.md");
		writeFileSync(
			planPath,
			"# Flow-Pair Plan\n\n## Phase 4: CLI Test\n\nPhase 4 fixture content.\n",
			"utf8",
		);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		writeFileSync(join(tasksDir, "tasks.md"), "# Tasks\n\n- [ ] T001 fixture task\n", "utf8");

		// Step 3: run the CLI via tsx (same mechanism as npm-linked `flow-pair` binary)
		const cliPath = join(process.cwd(), "skills/flow-pair/lib/cli.ts");
		const proc = spawnSync(
			"npx",
			[
				"tsx",
				cliPath,
				"dispatch",
				"--run-id",
				runId,
				"--plan-path",
				planPath,
				"--phase",
				"Phase 4: CLI Test",
				"--tasks-dir",
				tasksDir,
				"--ledger-root",
				ledgerRoot,
				"--repo",
				tmpRoot,
			],
			{ cwd: process.cwd(), encoding: "utf8", timeout: 30_000 },
		);

		expect(proc.status).toBe(0);

		// Step 4: stdout = EXACTLY the pointer line
		const nonEmptyLines = proc.stdout.split("\n").filter((l) => l.trim());
		expect(nonEmptyLines).toHaveLength(1);

		const line = nonEmptyLines[0];
		if (!line) throw new Error("dispatch stdout was empty");
		// Matches "[flow-pair dlg-NNNN] Packet at: <path>"
		expect(line).toMatch(/^\[flow-pair dlg-\d{4}\] Packet at: /);
		// No metadata fields
		expect(line).not.toMatch(/^pointerMsg:/);
		expect(line).not.toMatch(/^ok:/);
		expect(line).not.toMatch(/^delegationId:/);
		expect(line).not.toMatch(/^packId:/);
	}, 40_000); // tsx startup can be slow
});
