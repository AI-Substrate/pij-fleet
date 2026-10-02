// skills/flow-pair/test/ledger-records.test.ts
// T002: Failing tests for LedgerWriter record writers.
// Uses pre-fabricated runId + manual dir setup to avoid dependency on T003 impl.

import { mkdirSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { type LedgerDeps, LedgerWriter, nodeLedgerDeps } from "../lib/ledger.js";

// ─── Tracking deps ────────────────────────────────────────────────────────────

interface TrackingDeps extends LedgerDeps {
	callLog: string[];
}

function makeTrackingDeps(): TrackingDeps {
	const real = nodeLedgerDeps();
	const callLog: string[] = [];
	return {
		callLog,
		mkdirSync: (path, opts) => real.mkdirSync(path, opts),
		writeFileSync: (path, data) => {
			callLog.push("writeFileSync");
			real.writeFileSync(path, data);
		},
		appendFileSync: (path, data) => {
			callLog.push("appendFileSync");
			real.appendFileSync(path, data);
		},
		readFileSync: (path, enc) => real.readFileSync(path, enc),
		existsSync: (path) => real.existsSync(path),
		readdirSync: (path) => real.readdirSync(path),
	};
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

const FIXED_RUN_ID = "2026-06-17T00-00-00Z-test-repo-fixed";

/** Manually scaffold a run dir in tmpRoot/runs/<runId>/ without calling createRun. */
function scaffoldRunDir(tmpRoot: string, runId: string): string {
	const runDir = join(tmpRoot, "runs", runId);
	for (const subdir of ["", "delegations", "prompt-trials", "reviews", "learnings"]) {
		mkdirSync(join(runDir, subdir), { recursive: true });
	}
	// Seed an empty events.jsonl so appendFileSync can append to it
	writeFileSync(join(runDir, "events.jsonl"), "");
	return runDir;
}

// ─── Test suite ───────────────────────────────────────────────────────────────

describe("LedgerWriter — record writers", () => {
	let tmpRoot: string;
	let runDir: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-ledger-rec-"));
		runDir = scaffoldRunDir(tmpRoot, FIXED_RUN_ID);
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	// ─── writeDelegation ──────────────────────────────────────────────────────

	it("writeDelegation writes delegations/dlg-0001.json with correct fields", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const result = writer.writeDelegation(FIXED_RUN_ID, {
			taskRef: "T001",
			packetPath: "prompts/dlg-0001.md",
		});
		expect(result.ok).toBe(true);
		const rec = JSON.parse(
			deps.readFileSync(join(runDir, "delegations", "dlg-0001.json"), "utf8"),
		) as Record<string, unknown>;
		expect(rec.delegationId).toBe("dlg-0001");
		expect(rec.runId).toBe(FIXED_RUN_ID);
		expect(rec.taskRef).toBe("T001");
		expect(rec.packetPath).toBe("prompts/dlg-0001.md");
		expect(rec.status).toBe("pending");
	});

	it("writeDelegation appends delegation.created event", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		writer.writeDelegation(FIXED_RUN_ID, { taskRef: "T001", packetPath: "p.md" });
		const lines = deps
			.readFileSync(join(runDir, "events.jsonl"), "utf8")
			.split("\n")
			.filter(Boolean);
		expect(lines.length).toBeGreaterThanOrEqual(1);
		const event = JSON.parse(lines.at(-1) ?? "{}") as Record<string, unknown>;
		expect(event.type).toBe("delegation.created");
		expect(event.delegationId).toBe("dlg-0001");
	});

	it("writeDelegation ids are monotonic: dlg-0001, dlg-0002", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const r1 = writer.writeDelegation(FIXED_RUN_ID, { taskRef: "T001", packetPath: "p1.md" });
		const r2 = writer.writeDelegation(FIXED_RUN_ID, { taskRef: "T002", packetPath: "p2.md" });
		expect(r1.delegation?.delegationId).toBe("dlg-0001");
		expect(r2.delegation?.delegationId).toBe("dlg-0002");
	});

	it("P9: appendFileSync called before writeFileSync in writeDelegation", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		writer.writeDelegation(FIXED_RUN_ID, { taskRef: "T001", packetPath: "p.md" });
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});

	// ─── writePromptTrial ─────────────────────────────────────────────────────

	it("writePromptTrial writes prompt-trials/trial-0001.json with delegationId", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const result = writer.writePromptTrial(FIXED_RUN_ID, "dlg-0001", {
			templateRef: "templates/worker-implement.md",
			promptHash: "abc12345",
		});
		expect(result.ok).toBe(true);
		const rec = JSON.parse(
			deps.readFileSync(join(runDir, "prompt-trials", "trial-0001.json"), "utf8"),
		) as Record<string, unknown>;
		expect(rec.trialId).toBe("trial-0001");
		expect(rec.runId).toBe(FIXED_RUN_ID);
		expect(rec.delegationId).toBe("dlg-0001");
		expect(rec.promptHash).toBe("abc12345");
	});

	// ─── writeReview ──────────────────────────────────────────────────────────

	it("writeReview writes reviews/rev-0001.json with verdict and delegationId", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const result = writer.writeReview(FIXED_RUN_ID, "dlg-0001", {
			verdict: "APPROVE",
			findings: [{ dimension: "correctness", severity: "info", message: "looks good" }],
		});
		expect(result.ok).toBe(true);
		const rec = JSON.parse(
			deps.readFileSync(join(runDir, "reviews", "rev-0001.json"), "utf8"),
		) as Record<string, unknown>;
		expect(rec.reviewId).toBe("rev-0001");
		expect(rec.runId).toBe(FIXED_RUN_ID);
		expect(rec.delegationId).toBe("dlg-0001");
		expect(rec.verdict).toBe("APPROVE");
		expect(Array.isArray(rec.findings)).toBe(true);
	});

	// ─── writeLearning ────────────────────────────────────────────────────────

	it("writeLearning writes learnings/learn-0001.json with cluster and delegationId", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const result = writer.writeLearning(FIXED_RUN_ID, "dlg-0001", {
			cluster: "tdd-patterns",
			candidatePath: "prompt-lab/clusters/tdd-patterns/candidates/v1.md",
		});
		expect(result.ok).toBe(true);
		const rec = JSON.parse(
			deps.readFileSync(join(runDir, "learnings", "learn-0001.json"), "utf8"),
		) as Record<string, unknown>;
		expect(rec.learningId).toBe("learn-0001");
		expect(rec.runId).toBe(FIXED_RUN_ID);
		expect(rec.delegationId).toBe("dlg-0001");
		expect(rec.cluster).toBe("tdd-patterns");
	});
});
