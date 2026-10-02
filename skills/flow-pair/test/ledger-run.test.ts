// skills/flow-pair/test/ledger-run.test.ts
// T001 + F2/F3 fixes: run lifecycle tests, closeRun validation guard, collision guard.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { type LedgerDeps, LedgerWriter, nodeLedgerDeps } from "../lib/ledger.js";

// ─── Tracking deps (wraps real fs + records call order) ───────────────────────

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

// Helper: assert result.ok and extract run or throw
function assertRun(result: {
	ok: boolean;
	run?: { runId: string; runDir: string; repoId: string; status: string };
	error?: string;
}) {
	expect(result.ok).toBe(true);
	if (!result.run) throw new Error("expected result.run to be defined");
	return result.run;
}

// ─── Test suite ───────────────────────────────────────────────────────────────

describe("LedgerWriter — run lifecycle", () => {
	let tmpRoot: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-ledger-run-"));
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("createRun returns {ok:true} with runId, repoId, status:open", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const result = writer.createRun("github.com-foo-bar");
		expect(result.ok).toBe(true);
		expect(result.run?.repoId).toBe("github.com-foo-bar");
		expect(result.run?.status).toBe("open");
		expect(result.run?.runId).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}Z-/);
	});

	it("createRun writes run.json with correct shape", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const run = assertRun(writer.createRun("test-repo"));
		const runJson = JSON.parse(deps.readFileSync(join(run.runDir, "run.json"), "utf8")) as Record<
			string,
			unknown
		>;
		expect(runJson.status).toBe("open");
		expect(runJson.repoId).toBe("test-repo");
		expect(typeof runJson.runId).toBe("string");
		expect(typeof runJson.createdAt).toBe("string");
		expect(typeof runJson.runDir).toBe("string");
	});

	it("createRun scaffolds 7 subdirectories", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const run = assertRun(writer.createRun("test-repo"));
		for (const subdir of [
			"delegations",
			"prompt-trials",
			"reviews",
			"learnings",
			"prompts",
			"worker-reports",
			"diffs",
		]) {
			expect(deps.existsSync(join(run.runDir, subdir))).toBe(true);
		}
	});

	it("createRun appends run.started event to events.jsonl", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const run = assertRun(writer.createRun("test-repo"));
		const lines = deps
			.readFileSync(join(run.runDir, "events.jsonl"), "utf8")
			.split("\n")
			.filter(Boolean);
		expect(lines).toHaveLength(1);
		const event = JSON.parse(lines[0] ?? "{}") as Record<string, unknown>;
		expect(event.type).toBe("run.started");
		expect(event.runId).toBe(run.runId);
		expect(event.repoId).toBe("test-repo");
		expect(typeof event.at).toBe("string");
	});

	it("P9: appendFileSync called before writeFileSync in createRun", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		writer.createRun("test-repo");
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});

	it("closeRun appends run.closed and updates run.json status to closed", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const run = assertRun(writer.createRun("test-repo"));

		const closeResult = writer.closeRun(run.runId);
		expect(closeResult.ok).toBe(true);

		const lines = deps
			.readFileSync(join(run.runDir, "events.jsonl"), "utf8")
			.split("\n")
			.filter(Boolean);
		const lastEvent = JSON.parse(lines.at(-1) ?? "{}") as Record<string, unknown>;
		expect(lastEvent.type).toBe("run.closed");

		const runJson = JSON.parse(deps.readFileSync(join(run.runDir, "run.json"), "utf8")) as Record<
			string,
			unknown
		>;
		expect(runJson.status).toBe("closed");
		expect(typeof runJson.closedAt).toBe("string");
	});

	it("P9: appendFileSync called before writeFileSync in closeRun", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(tmpRoot, deps);
		const run = assertRun(writer.createRun("test-repo"));
		deps.callLog.length = 0; // isolate closeRun calls
		writer.closeRun(run.runId);
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});
});

// ─── F2: closeRun validates run.json before appending ────────────────────────

describe("LedgerWriter — closeRun validation (F2)", () => {
	let tmpRoot: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-close-guard-"));
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("closeRun returns {ok:false} and does NOT append run.closed when run.json missing", () => {
		const writer = new LedgerWriter(tmpRoot, nodeLedgerDeps());
		// Scaffold run dir WITHOUT run.json
		const runId = "2026-06-17T00-00-00Z-no-runjson";
		const runDir = join(tmpRoot, "runs", runId);
		mkdirSync(runDir, { recursive: true });
		writeFileSync(join(runDir, "events.jsonl"), "");

		const result = writer.closeRun(runId);
		expect(result.ok).toBe(false);

		// events.jsonl must still be empty — no false run.closed appended
		const events = readFileSync(join(runDir, "events.jsonl"), "utf8").split("\n").filter(Boolean);
		expect(events).toHaveLength(0);
	});

	it("closeRun returns {ok:false} and does NOT append run.closed when run.json malformed", () => {
		const writer = new LedgerWriter(tmpRoot, nodeLedgerDeps());
		const runId = "2026-06-17T00-00-00Z-bad-runjson";
		const runDir = join(tmpRoot, "runs", runId);
		mkdirSync(runDir, { recursive: true });
		writeFileSync(join(runDir, "events.jsonl"), "");
		writeFileSync(join(runDir, "run.json"), "not-valid-json{{{");

		const result = writer.closeRun(runId);
		expect(result.ok).toBe(false);

		const events = readFileSync(join(runDir, "events.jsonl"), "utf8").split("\n").filter(Boolean);
		expect(events).toHaveLength(0);
	});
});

// ─── F3: createRun collision guard ───────────────────────────────────────────

describe("LedgerWriter — createRun collision guard (F3)", () => {
	it("createRun returns {ok:false} without writing when runDir already exists", () => {
		// Use a fake existsSync that always reports the runDir as already existing
		let writeWasCalled = false;
		let appendWasCalled = false;
		const deps: LedgerDeps = {
			existsSync: () => true, // runDir always "exists"
			mkdirSync: () => {},
			writeFileSync: () => {
				writeWasCalled = true;
			},
			appendFileSync: () => {
				appendWasCalled = true;
			},
			readFileSync: () => "",
			readdirSync: () => [],
		};
		const writer = new LedgerWriter("/tmp/any", deps);
		const result = writer.createRun("test-repo");
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/already exists/);
		expect(writeWasCalled).toBe(false);
		expect(appendWasCalled).toBe(false);
	});
});
