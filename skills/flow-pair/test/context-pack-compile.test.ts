// skills/flow-pair/test/context-pack-compile.test.ts
// T003: compile — error branches (3 tests)
// T004: compile — manifest assembly (7 tests)
// T005: compile — P9 invariant + cluster isolation (3 tests)
// All tests go RED against the stub.

import { mkdirSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	ContextPackCompiler,
	type ContextPackDeps,
	DEFAULT_FORBIDDEN_PATHS,
	nodeContextPackDeps,
} from "../lib/context-pack.js";

// ─── Fixture helpers ──────────────────────────────────────────────────────────

/** Scaffold a minimal run dir (events.jsonl + context-packs subdir for re-runs). */
function scaffoldRun(ledgerRoot: string, runId: string): string {
	const runDir = join(ledgerRoot, "runs", runId);
	mkdirSync(runDir, { recursive: true });
	writeFileSync(join(runDir, "events.jsonl"), "", "utf8");
	return runDir;
}

/** Create a minimal plan with a named phase section. */
function makePlan(tmpRoot: string, phases: { heading: string; body: string }[]): string {
	const planPath = join(tmpRoot, "plan.md");
	const lines: string[] = ["# Flow-Pair Plan", ""];
	for (const { heading, body } of phases) {
		lines.push(`## ${heading}`, body, "");
	}
	writeFileSync(planPath, lines.join("\n"), "utf8");
	return planPath;
}

// ─── Tracking deps ───────────────────────────────────────────────────────────

interface TrackingDeps extends ContextPackDeps {
	callLog: string[];
}

function makeTrackingDeps(): TrackingDeps {
	const real = nodeContextPackDeps();
	const callLog: string[] = [];
	return {
		callLog,
		readFileSync: (p, e) => real.readFileSync(p, e),
		existsSync: (p) => real.existsSync(p),
		readdirSync: (p) => real.readdirSync(p),
		mkdirSync: (p, o) => real.mkdirSync(p, o),
		writeFileSync: (p, d) => {
			callLog.push("writeFileSync");
			real.writeFileSync(p, d);
		},
		appendFileSync: (p, d) => {
			callLog.push("appendFileSync");
			real.appendFileSync(p, d);
		},
	};
}

/** Deps that wrap real fs but make appendFileSync throw. Tracks whether writeFileSync was called. */
function makeFailAppendDeps(): ContextPackDeps & { writeWasCalled: boolean } {
	const real = nodeContextPackDeps();
	let writeWasCalled = false;
	return {
		get writeWasCalled() {
			return writeWasCalled;
		},
		readFileSync: (p, e) => real.readFileSync(p, e),
		existsSync: (p) => real.existsSync(p),
		readdirSync: (p) => real.readdirSync(p),
		mkdirSync: (p, o) => real.mkdirSync(p, o),
		writeFileSync: () => {
			writeWasCalled = true;
		},
		appendFileSync: () => {
			throw new Error("disk full — injected failure");
		},
	};
}

// ─── T003: compile — error branches ──────────────────────────────────────────

describe("ContextPackCompiler.compile — T003: error branches", () => {
	let tmpRoot: string;
	let ledgerRoot: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-compile-err-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		mkdirSync(ledgerRoot, { recursive: true });
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("returns {ok:false} when plan file does not exist (mutation guard: remove file-missing check → RED)", () => {
		const runId = "2026-06-17T00-00-00Z-t003-a";
		scaffoldRun(ledgerRoot, runId);
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath: join(tmpRoot, "no-plan.md"),
			phase: "Phase 3: Compiler",
			tasksDir: tmpRoot,
			cluster: "implement-code",
			allowedPaths: ["skills/flow-pair/lib/context-pack.ts"],
		});
		expect(result.ok).toBe(false);
		expect(result.error).toBeDefined();
	});

	it("returns {ok:false} with error containing 'section not found' when phase missing from plan (mutation guard: remove section-missing propagation → RED)", () => {
		const runId = "2026-06-17T00-00-00Z-t003-b";
		scaffoldRun(ledgerRoot, runId);
		const planPath = makePlan(tmpRoot, [
			{ heading: "Phase 1: Identity", body: "Identity content." },
		]);
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 99: Nonexistent",
			tasksDir: tmpRoot,
			cluster: "implement-code",
			allowedPaths: [],
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/section not found/i);
	});

	it("returns {ok:false} for invalid runId containing '..' (resolveRunDir guard propagated)", () => {
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const result = compiler.compile({
			runId: "../escape",
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir: tmpRoot,
			cluster: "implement-code",
			allowedPaths: [],
		});
		expect(result.ok).toBe(false);
		expect(result.error).toBeDefined();
	});
});

// ─── T004: compile — manifest assembly ───────────────────────────────────────

describe("ContextPackCompiler.compile — T004: manifest assembly", () => {
	let tmpRoot: string;
	let ledgerRoot: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-compile-mfst-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		mkdirSync(ledgerRoot, { recursive: true });
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	function baseCompile(compiler: ContextPackCompiler, runId: string, tasksDir: string) {
		const planPath = makePlan(tmpRoot, [
			{ heading: "Phase 3: Compiler", body: "Plan section content." },
		]);
		return compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: ["skills/flow-pair/lib/context-pack.ts"],
		});
	}

	it("manifest packId matches ^cp-\\d{4}$ format (mutation guard: remove padStart → RED)", () => {
		const runId = "2026-06-17T00-00-00Z-t004-a";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = baseCompile(compiler, runId, tasksDir);
		expect(result.ok).toBe(true);
		expect(result.manifest?.packId).toMatch(/^cp-\d{4}$/);
	});

	it("manifest plan-phase entry has non-empty hash and content", () => {
		const runId = "2026-06-17T00-00-00Z-t004-b";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = baseCompile(compiler, runId, tasksDir);
		expect(result.ok).toBe(true);
		const planEntry = result.manifest?.entries.find((e) => e.role === "plan-phase");
		expect(planEntry).toBeDefined();
		expect(planEntry?.hash).toMatch(/^[0-9a-f]{8}$/);
		expect(planEntry?.content).toContain("Plan section content.");
	});

	it("manifest includes tasks entry when tasks.md exists in tasksDir", () => {
		const runId = "2026-06-17T00-00-00Z-t004-c";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		writeFileSync(join(tasksDir, "tasks.md"), "# Tasks\nT001 done.", "utf8");
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = baseCompile(compiler, runId, tasksDir);
		expect(result.ok).toBe(true);
		expect(result.manifest?.entries.some((e) => e.role === "tasks")).toBe(true);
	});

	it("execution-log excluded with reason 'not found' when absent (mutation guard: remove exclusion-recording → RED)", () => {
		const runId = "2026-06-17T00-00-00Z-t004-d";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		// NO execution.log.md in tasksDir
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = baseCompile(compiler, runId, tasksDir);
		expect(result.ok).toBe(true);
		const excl = result.manifest?.exclusions.find((e) => e.path.includes("execution.log.md"));
		expect(excl).toBeDefined();
		expect(excl?.reason).toBe("not found");
	});

	it("manifest allowedPaths matches opts.allowedPaths", () => {
		const runId = "2026-06-17T00-00-00Z-t004-e";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: [
				"skills/flow-pair/lib/context-pack.ts",
				"skills/flow-pair/test/context-pack-extract.test.ts",
			],
		});
		expect(result.ok).toBe(true);
		expect(result.manifest?.allowedPaths).toEqual([
			"skills/flow-pair/lib/context-pack.ts",
			"skills/flow-pair/test/context-pack-extract.test.ts",
		]);
	});

	it("manifest forbiddenPaths includes DEFAULT_FORBIDDEN_PATHS when not overridden (mutation guard: remove default → RED)", () => {
		const runId = "2026-06-17T00-00-00Z-t004-f";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = baseCompile(compiler, runId, tasksDir);
		expect(result.ok).toBe(true);
		for (const forbidden of DEFAULT_FORBIDDEN_PATHS) {
			expect(result.manifest?.forbiddenPaths).toContain(forbidden);
		}
	});

	it("manifest delegationId matches opts.delegationId", () => {
		const runId = "2026-06-17T00-00-00Z-t004-g";
		scaffoldRun(ledgerRoot, runId);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot);
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0042",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: [],
		});
		expect(result.ok).toBe(true);
		expect(result.manifest?.delegationId).toBe("dlg-0042");
	});
});

// ─── T005: compile — P9 invariant + cluster isolation ────────────────────────

describe("ContextPackCompiler.compile — T005: P9 + cluster isolation", () => {
	let tmpRoot: string;
	let ledgerRoot: string;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-compile-p9-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		mkdirSync(ledgerRoot, { recursive: true });
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("P9: appendFileSync called before writeFileSync in compile (mutation guard: swap order → RED)", () => {
		const deps = makeTrackingDeps();
		const runId = "2026-06-17T00-00-00Z-t005-a";
		scaffoldRun(ledgerRoot, runId);
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot, deps);
		compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: [],
		});
		const appendIdx = deps.callLog.indexOf("appendFileSync");
		const writeIdx = deps.callLog.indexOf("writeFileSync");
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThan(appendIdx);
	});

	it("P9 failure injection: appendFileSync throws → {ok:false} + writeFileSync NOT called (mutation guard: remove failure propagation → RED)", () => {
		const deps = makeFailAppendDeps();
		const runId = "2026-06-17T00-00-00Z-t005-b";
		scaffoldRun(ledgerRoot, runId);
		// Compiler uses our FailAppendDeps — wraps real fs but appendFileSync throws
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot, deps);
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: [],
		});
		expect(result.ok).toBe(false);
		expect(deps.writeWasCalled).toBe(false);
	});

	it("cluster isolation: active.md from cluster-A is NOT in entries when compiling for cluster-B (mutation guard: remove cluster filter → RED)", () => {
		// DL-003: clusters live under the skill root — scaffold cluster-A there.
		const skillRoot = join(tmpRoot, "skill");
		const clusterADir = join(skillRoot, "prompt-lab/clusters/cluster-a");
		mkdirSync(clusterADir, { recursive: true });
		writeFileSync(join(clusterADir, "active.md"), "Cluster A learnings.", "utf8");

		const runId = "2026-06-17T00-00-00Z-t005-c";
		scaffoldRun(ledgerRoot, runId);
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot, undefined, skillRoot);
		// Compile for cluster-B, not cluster-A
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "cluster-b",
			allowedPaths: [],
		});
		expect(result.ok).toBe(true);
		// No learning entry from cluster-A
		const learningEntries = result.manifest?.entries.filter((e) => e.role === "learning") ?? [];
		for (const entry of learningEntries) {
			expect(entry.content).not.toContain("Cluster A learnings.");
		}
		// Also: no cluster-a in entry paths
		const entryPaths = result.manifest?.entries.map((e) => e.path) ?? [];
		expect(entryPaths.every((p) => !p.includes("cluster-a"))).toBe(true);
	});

	it("DL-003: learnings compile from skillRoot, NOT from a repoRoot-joined layout (mutation guard: rejoin repoRoot → RED)", () => {
		const skillRoot = join(tmpRoot, "skill");
		// Correct location — under the skill root
		const skillClusterDir = join(skillRoot, "prompt-lab/clusters/implement-code");
		mkdirSync(skillClusterDir, { recursive: true });
		writeFileSync(join(skillClusterDir, "active.md"), "SKILL-ROOT learnings.", "utf8");
		// Decoy at the old (buggy) repoRoot-joined location — must be ignored
		const repoClusterDir = join(tmpRoot, "skills/flow-pair/prompt-lab/clusters/implement-code");
		mkdirSync(repoClusterDir, { recursive: true });
		writeFileSync(join(repoClusterDir, "active.md"), "REPO-ROOT learnings — WRONG.", "utf8");

		const runId = "2026-06-17T00-00-00Z-t005-d";
		scaffoldRun(ledgerRoot, runId);
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot, undefined, skillRoot);
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: [],
		});
		expect(result.ok).toBe(true);
		const learningEntries = result.manifest?.entries.filter((e) => e.role === "learning") ?? [];
		expect(learningEntries).toHaveLength(1);
		expect(learningEntries[0]?.content).toBe("SKILL-ROOT learnings.");
		expect(learningEntries[0]?.path).toContain(skillRoot);
	});

	it("DL-003 honesty: absent cluster learnings are recorded as a manifest exclusion, not silently dropped (mutation guard: remove exclusion push → RED)", () => {
		const skillRoot = join(tmpRoot, "skill");
		mkdirSync(skillRoot, { recursive: true }); // no prompt-lab at all

		const runId = "2026-06-17T00-00-00Z-t005-e";
		scaffoldRun(ledgerRoot, runId);
		const planPath = makePlan(tmpRoot, [{ heading: "Phase 3: Compiler", body: "Content." }]);
		const tasksDir = join(tmpRoot, "tasks");
		mkdirSync(tasksDir, { recursive: true });
		const compiler = new ContextPackCompiler(tmpRoot, ledgerRoot, undefined, skillRoot);
		const result = compiler.compile({
			runId,
			delegationId: "dlg-0001",
			planPath,
			phase: "Phase 3: Compiler",
			tasksDir,
			cluster: "implement-code",
			allowedPaths: [],
		});
		expect(result.ok).toBe(true);
		expect(result.manifest?.entries.some((e) => e.role === "learning")).toBe(false);
		const excl = result.manifest?.exclusions.find((e) =>
			e.path.endsWith(join("clusters", "implement-code", "active.md")),
		);
		expect(excl).toBeDefined();
		expect(excl?.reason).toBe("not found");
		expect(excl?.path).toContain(skillRoot);
	});
});
