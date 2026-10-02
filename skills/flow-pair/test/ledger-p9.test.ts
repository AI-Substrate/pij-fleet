// skills/flow-pair/test/ledger-p9.test.ts
// F1 + F4: P9 failure-injection tests for all 6 LedgerWriter methods.
// Injects an appendFileSync that throws and asserts:
//   (a) writeFileSync is NOT called
//   (b) the public method returns {ok:false}
// Also covers success-path P9 order for the 3 record writers not in ledger-records.test.ts.

import { mkdirSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { type LedgerDeps, LedgerWriter, nodeLedgerDeps, type RunRecord } from "../lib/ledger.js";

// ─── Fixed test constants ─────────────────────────────────────────────────────

const FIXED_RUN_ID = "2026-06-17T00-00-00Z-p9-test-fixed";

/** A minimal valid RunRecord JSON for closeRun read-step. */
function makeRunJson(runId: string): string {
	const rec: RunRecord = {
		runId,
		repoId: "test-repo",
		runDir: ".",
		createdAt: new Date().toISOString(),
		status: "open",
	};
	return JSON.stringify(rec);
}

// ─── Failure-injection deps ───────────────────────────────────────────────────

/** Deps where appendFileSync always throws. Records whether writeFileSync was called. */
interface FailDeps extends LedgerDeps {
	writeWasCalled: boolean;
}

function makeFailAppendDeps(runId: string): FailDeps {
	let writeWasCalled = false;
	const deps: FailDeps = {
		get writeWasCalled() {
			return writeWasCalled;
		},
		mkdirSync: () => {},
		writeFileSync: () => {
			writeWasCalled = true;
		},
		appendFileSync: () => {
			throw new Error("disk full — injected failure");
		},
		// readFileSync returns valid run.json so closeRun's validation step passes
		readFileSync: () => makeRunJson(runId),
		existsSync: () => false, // no collision
		readdirSync: () => [],
	};
	return deps;
}

// ─── Tracking deps (wraps real fs for success-path P9 order tests) ─────────────

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

/** Manually scaffold a run dir with events.jsonl and optional run.json. */
function scaffoldRunDir(tmpRoot: string, runId: string, withRunJson = false): string {
	const runDir = join(tmpRoot, "runs", runId);
	for (const sub of ["", "delegations", "prompt-trials", "reviews", "learnings"]) {
		mkdirSync(join(runDir, sub), { recursive: true });
	}
	writeFileSync(join(runDir, "events.jsonl"), "");
	if (withRunJson) {
		writeFileSync(join(runDir, "run.json"), makeRunJson(runId));
	}
	return runDir;
}

// ─── Failure-injection tests: F1 (all 6 writers) ─────────────────────────────

describe("LedgerWriter — P9 failure injection (F1)", () => {
	it("createRun: appendFileSync throws → {ok:false} + writeFileSync NOT called", () => {
		const deps = makeFailAppendDeps(FIXED_RUN_ID);
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.createRun("test-repo");
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/disk full|failed to append/);
		expect(deps.writeWasCalled).toBe(false);
	});

	it("closeRun: appendFileSync throws → {ok:false} + writeFileSync NOT called", () => {
		const deps = makeFailAppendDeps(FIXED_RUN_ID);
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.closeRun(FIXED_RUN_ID);
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/disk full|failed to append/);
		expect(deps.writeWasCalled).toBe(false);
	});

	it("writeDelegation: appendFileSync throws → {ok:false} + writeFileSync NOT called", () => {
		const deps = makeFailAppendDeps(FIXED_RUN_ID);
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.writeDelegation(FIXED_RUN_ID, { taskRef: "T001", packetPath: "p.md" });
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/disk full|failed to append/);
		expect(deps.writeWasCalled).toBe(false);
	});

	it("writePromptTrial: appendFileSync throws → {ok:false} + writeFileSync NOT called", () => {
		const deps = makeFailAppendDeps(FIXED_RUN_ID);
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.writePromptTrial(FIXED_RUN_ID, "dlg-0001", {
			templateRef: "t.md",
			promptHash: "abc",
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/disk full|failed to append/);
		expect(deps.writeWasCalled).toBe(false);
	});

	it("writeReview: appendFileSync throws → {ok:false} + writeFileSync NOT called", () => {
		const deps = makeFailAppendDeps(FIXED_RUN_ID);
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.writeReview(FIXED_RUN_ID, "dlg-0001", {
			verdict: "APPROVE",
			findings: [],
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/disk full|failed to append/);
		expect(deps.writeWasCalled).toBe(false);
	});

	it("writeLearning: appendFileSync throws → {ok:false} + writeFileSync NOT called", () => {
		const deps = makeFailAppendDeps(FIXED_RUN_ID);
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.writeLearning(FIXED_RUN_ID, "dlg-0001", {
			cluster: "c",
			candidatePath: "p.md",
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/disk full|failed to append/);
		expect(deps.writeWasCalled).toBe(false);
	});
});

// ─── Success-path P9 order tests for writers 4-6 (F4) ─────────────────────────

describe("LedgerWriter — P9 success-path order (F4: writePromptTrial/writeReview/writeLearning)", () => {
	let tmpRoot: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-p9-order-"));
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("P9: appendFileSync before writeFileSync in writePromptTrial", () => {
		const deps = makeTrackingDeps();
		scaffoldRunDir(tmpRoot, FIXED_RUN_ID);
		const writer = new LedgerWriter(tmpRoot, deps);
		writer.writePromptTrial(FIXED_RUN_ID, "dlg-0001", {
			templateRef: "t.md",
			promptHash: "abc",
		});
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});

	it("P9: appendFileSync before writeFileSync in writeReview", () => {
		const deps = makeTrackingDeps();
		scaffoldRunDir(tmpRoot, FIXED_RUN_ID);
		const writer = new LedgerWriter(tmpRoot, deps);
		writer.writeReview(FIXED_RUN_ID, "dlg-0001", { verdict: "APPROVE", findings: [] });
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});

	it("P9: appendFileSync before writeFileSync in writeLearning", () => {
		const deps = makeTrackingDeps();
		scaffoldRunDir(tmpRoot, FIXED_RUN_ID);
		const writer = new LedgerWriter(tmpRoot, deps);
		writer.writeLearning(FIXED_RUN_ID, "dlg-0001", {
			cluster: "c",
			candidatePath: "p.md",
		});
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});
});
