// skills/flow-pair/test/observe.test.ts
// T001: basic diff artifacts — 6 tests (HIGH-B: untracked via porcelain)
// T002: flow-state guard AC-13 — 4 tests (HIGH-A: nested path; HIGH-B: untracked forbidden)
// T003: P9 ordering invariant — 4 tests

import { execSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { removeTemporaryTree } from "../../../harness/test-utils.js";
import { nodeObserveDeps, Observe, type ObserveDeps } from "../lib/observe.js";

// ─── Fixture helpers ──────────────────────────────────────────────────────────

/** Create a minimal git fixture: init → commit → staged change + one untracked file. */
function makeGitFixture(root: string): { repoDir: string } {
	const repoDir = join(root, "repo");
	mkdirSync(repoDir, { recursive: true });
	execSync("git init", { cwd: repoDir, encoding: "utf8" });
	execSync("git config user.email ci@test.com", { cwd: repoDir });
	execSync("git config user.name CI", { cwd: repoDir });
	writeFileSync(join(repoDir, "README.md"), "# Test\n");
	execSync("git add README.md", { cwd: repoDir });
	execSync("git commit -m init", { cwd: repoDir });
	// Staged change — appears in git diff HEAD AND git status --porcelain
	writeFileSync(join(repoDir, "src.ts"), "export const x = 1;\n");
	execSync("git add src.ts", { cwd: repoDir });
	// Untracked file — ONLY in git status --porcelain, NOT in git diff HEAD --name-only
	writeFileSync(join(repoDir, "untracked.ts"), "export const y = 2;\n");
	return { repoDir };
}

/** Scaffold the minimum ledger run structure needed for capture(). */
function scaffoldLedger(ledgerRoot: string, runId: string): void {
	const runDir = join(ledgerRoot, "runs", runId);
	mkdirSync(join(runDir, "diffs"), { recursive: true });
}

// ─── Tracking deps (P3 helpers) ───────────────────────────────────────────────

type CallEntry = { method: "appendFileSync" | "writeFileSync"; path: string };

/** Wraps real deps, records every appendFileSync + writeFileSync call in order. */
class TrackingDeps implements ObserveDeps {
	readonly callLog: CallEntry[] = [];
	appendWasCalled = false;
	writeWasCalled = false;

	constructor(private readonly real: ObserveDeps) {}

	execGit(args: string[], cwd: string) {
		return this.real.execGit(args, cwd);
	}
	writeFileSync(path: string, data: string): void {
		this.writeWasCalled = true;
		this.callLog.push({ method: "writeFileSync", path });
		this.real.writeFileSync(path, data);
	}
	appendFileSync(path: string, data: string): void {
		this.appendWasCalled = true;
		this.callLog.push({ method: "appendFileSync", path });
		this.real.appendFileSync(path, data);
	}
	mkdirSync(path: string, opts: { recursive: boolean }) {
		this.real.mkdirSync(path, opts);
	}
	existsSync(path: string) {
		return this.real.existsSync(path);
	}
	readdirSync(path: string) {
		return this.real.readdirSync(path);
	}
}

/**
 * Vacuous-trap-safe FailDeps: wraps real git + real fs reads, throws ONLY on appendFileSync.
 * FailDeps reaches the P9 guard line via real git (not a stub) so the RED is genuine.
 * writeFileSync tracks the call (does NOT write) — if P9 guard fires, writeWasCalled stays false.
 */
class FailDeps implements ObserveDeps {
	writeWasCalled = false;

	constructor(private readonly real: ObserveDeps) {}

	execGit(args: string[], cwd: string) {
		return this.real.execGit(args, cwd);
	}
	writeFileSync(_path: string, _data: string): void {
		this.writeWasCalled = true;
		// Don't call real — tracking only; the assertion is whether this is reached.
	}
	appendFileSync(_path: string, _data: string): void {
		throw new Error("injected appendFileSync failure");
	}
	mkdirSync(path: string, opts: { recursive: boolean }) {
		this.real.mkdirSync(path, opts);
	}
	existsSync(path: string) {
		return this.real.existsSync(path);
	}
	readdirSync(path: string) {
		return this.real.readdirSync(path);
	}
}

// ─── T001: basic diff artifacts ───────────────────────────────────────────────

describe("Observe — basic diff artifacts (T001)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let repoDir: string;
	const RUN_ID = "2026-06-18T00-00-00Z-obsv-t001-xxx";

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "observe-t001-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		scaffoldLedger(ledgerRoot, RUN_ID);
		({ repoDir } = makeGitFixture(tmpRoot));
	});

	afterEach(() => {
		removeTemporaryTree(tmpRoot);
	});

	it("returns {ok:true} with a real git fixture", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		expect(result.ok).toBe(true);
	});

	it("writes diff-0001.patch (non-empty)", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		const patchPath = join(ledgerRoot, "runs", RUN_ID, "diffs", "diff-0001.patch");
		expect(existsSync(patchPath)).toBe(true);
		expect(readFileSync(patchPath, "utf8").trim().length).toBeGreaterThan(0);
	});

	it("writes diff-0001.stat.txt (non-empty)", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		const statPath = join(ledgerRoot, "runs", RUN_ID, "diffs", "diff-0001.stat.txt");
		expect(existsSync(statPath)).toBe(true);
		expect(readFileSync(statPath, "utf8").trim().length).toBeGreaterThan(0);
	});

	it("writes diff-0001.changed-files.json with required fields", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		expect(result.ok).toBe(true);
		const manifestPath = join(ledgerRoot, "runs", RUN_ID, "diffs", "diff-0001.changed-files.json");
		expect(existsSync(manifestPath)).toBe(true);
		const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as Record<string, unknown>;
		expect(manifest.diffId).toBe("diff-0001");
		expect(manifest.runId).toBe(RUN_ID);
		expect(manifest.delegationId).toBe("dlg-0001");
		expect(Array.isArray(manifest.changedFiles)).toBe(true);
		expect(typeof manifest.at).toBe("string");
	});

	it("result.changedFiles includes the staged file", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		expect(result.ok).toBe(true);
		const changedFiles = result.result?.changedFiles ?? [];
		expect(changedFiles).toContain("src.ts");
	});

	it("result.changedFiles includes the untracked file (HIGH-B: porcelain sourcing)", () => {
		// git diff HEAD --name-only would miss untracked.ts
		// git status --porcelain captures it as "?? untracked.ts"
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		expect(result.ok).toBe(true);
		const changedFiles = result.result?.changedFiles ?? [];
		expect(changedFiles).toContain("untracked.ts");
	});

	it("MED-2: .patch includes synthetic patch content for untracked file", () => {
		// git diff HEAD only covers staged/tracked changes; untracked.ts absent without fix.
		// git diff --no-index -- /dev/null untracked.ts (exit 1 = diffs found = expected success)
		// is appended to .patch so new files are reconstructable.
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		const patchPath = join(ledgerRoot, "runs", RUN_ID, "diffs", "diff-0001.patch");
		const patchContent = readFileSync(patchPath, "utf8");
		expect(patchContent).toContain("untracked.ts");
	});
});

// ─── T002: flow-state guard AC-13 ─────────────────────────────────────────────

describe("Observe — flow-state guard AC-13 (T002)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	const RUN_ID = "2026-06-18T00-00-00Z-obsv-t002-xxx";

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "observe-t002-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		scaffoldLedger(ledgerRoot, RUN_ID);
	});

	afterEach(() => {
		removeTemporaryTree(tmpRoot);
	});

	/** Create git repo, write forbidden file at filename (supports nested paths), optionally untracked. */
	function makeForbiddenFixture(filename: string, opts?: { untracked?: boolean }): string {
		const repoDir = join(tmpRoot, "repo");
		mkdirSync(repoDir, { recursive: true });
		execSync("git init", { cwd: repoDir, encoding: "utf8" });
		execSync("git config user.email ci@test.com", { cwd: repoDir });
		execSync("git config user.name CI", { cwd: repoDir });
		writeFileSync(join(repoDir, "README.md"), "# Test\n");
		execSync("git add README.md", { cwd: repoDir });
		execSync("git commit -m init", { cwd: repoDir });
		// Create the forbidden file — possibly nested
		const target = join(repoDir, filename);
		mkdirSync(dirname(target), { recursive: true });
		writeFileSync(target, "forbidden content\n");
		if (!opts?.untracked) {
			execSync(`git add "${filename}"`, { cwd: repoDir });
		}
		return repoDir;
	}

	function runCapture(repoDir: string) {
		const tracking = new TrackingDeps(nodeObserveDeps());
		const obs = new Observe(ledgerRoot, tracking);
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		return { result, tracking };
	}

	it("nested docs/plans/016-flow-pair/.the-flow-state.json staged → {ok:false} + no writes [HIGH-A load-bearing]", () => {
		// LOAD-BEARING: git reports "docs/plans/016-flow-pair/.the-flow-state.json"
		// A bare FLOW_STATE_FORBIDDEN.includes(f) would return false (wrong design)
		// Only basename(f) === ".the-flow-state.json" catches this correctly
		const repoDir = makeForbiddenFixture("docs/plans/016-flow-pair/.the-flow-state.json");
		const { result, tracking } = runCapture(repoDir);
		// Primary — flips RED under: if (forbidden) → if (false)
		expect(result.ok).toBe(false);
		// No-write — flips RED when execution continues past guard
		expect(tracking.appendWasCalled).toBe(false);
		expect(tracking.writeWasCalled).toBe(false);
	});

	it("root-level the-flow.json staged → {ok:false} + no writes", () => {
		const repoDir = makeForbiddenFixture("the-flow.json");
		const { result, tracking } = runCapture(repoDir);
		expect(result.ok).toBe(false);
		expect(tracking.appendWasCalled).toBe(false);
		expect(tracking.writeWasCalled).toBe(false);
	});

	it(".flow-pair/runs/x staged → {ok:false} + no writes [prefix guard]", () => {
		const repoDir = makeForbiddenFixture(".flow-pair/runs/x");
		const { result, tracking } = runCapture(repoDir);
		expect(result.ok).toBe(false);
		expect(tracking.appendWasCalled).toBe(false);
		expect(tracking.writeWasCalled).toBe(false);
	});

	it("untracked the-flow.md (not staged) → {ok:false} + no writes [HIGH-B guard-bypass test]", () => {
		// If changedFiles came from git diff HEAD --name-only, untracked the-flow.md
		// would be INVISIBLE → result.ok=true → test fails (proving HIGH-B is load-bearing)
		const repoDir = makeForbiddenFixture("the-flow.md", { untracked: true });
		const { result, tracking } = runCapture(repoDir);
		expect(result.ok).toBe(false);
		expect(tracking.appendWasCalled).toBe(false);
		expect(tracking.writeWasCalled).toBe(false);
	});

	it("space-in-dirname forbidden path → {ok:false} + no writes [HIGH-1: NUL-delimited parsing]", () => {
		// NON-VACUOUS PROOF:
		// git status --porcelain (without -z) QUOTES paths with spaces:
		//   A  "docs/plans/016 flow-pair/.the-flow-state.json"  (literal double-quotes)
		// Old parsePorcelain strips 3 chars but keeps quotes:
		//   basename = '.the-flow-state.json"' (trailing quote) -> NOT in FLOW_STATE_FORBIDDEN -> {ok:true}
		// With --porcelain=v1 -z, NUL-delimited, zero quoting:
		//   basename = '.the-flow-state.json' -> guard fires -> {ok:false}
		const repoDir = makeForbiddenFixture("docs/plans/016 flow-pair/.the-flow-state.json");
		const { result, tracking } = runCapture(repoDir);
		expect(result.ok).toBe(false);
		expect(tracking.appendWasCalled).toBe(false);
		expect(tracking.writeWasCalled).toBe(false);
	});
});

// ─── T003: P9 ordering invariant ──────────────────────────────────────────────

describe("Observe — P9 ordering invariant (T003)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let repoDir: string;
	const RUN_ID = "2026-06-18T00-00-00Z-obsv-t003-xxx";

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "observe-t003-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		scaffoldLedger(ledgerRoot, RUN_ID);
		({ repoDir } = makeGitFixture(tmpRoot));
	});

	afterEach(() => {
		removeTemporaryTree(tmpRoot);
	});

	it("P9: appendFileSync(events.jsonl) before any writeFileSync(diffs/)", () => {
		const tracking = new TrackingDeps(nodeObserveDeps());
		const obs = new Observe(ledgerRoot, tracking);
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		expect(result.ok).toBe(true);

		const appendIdx = tracking.callLog.findIndex(
			(e) => e.method === "appendFileSync" && e.path.endsWith("events.jsonl"),
		);
		const writeIdx = tracking.callLog.findIndex(
			(e) => e.method === "writeFileSync" && e.path.includes("/diffs/"),
		);

		expect(appendIdx).toBeGreaterThanOrEqual(0); // appendFileSync was called
		expect(writeIdx).toBeGreaterThanOrEqual(0); // writeFileSync was called
		expect(appendIdx).toBeLessThan(writeIdx); // append BEFORE write (P9)
	});

	it("P9 guard: FailDeps appendFileSync throws → {ok:false} + diff files NOT written", () => {
		// FailDeps wraps real git (not mocked) — reaches the P9 guard via real execution.
		// Throws only at appendFileSync (P9 step).
		// Mutation `if (!ev.ok) → if (false)` → writeWasCalled flips true → assertion RED ✓
		const failDeps = new FailDeps(nodeObserveDeps());
		const obs = new Observe(ledgerRoot, failDeps);
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		expect(result.ok).toBe(false);
		expect(failDeps.writeWasCalled).toBe(false); // no diff artifacts written
	});

	it("files.changed event in events.jsonl has required field set", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });

		const eventsPath = join(ledgerRoot, "runs", RUN_ID, "events.jsonl");
		const lines = readFileSync(eventsPath, "utf8")
			.split("\n")
			.filter((l) => l.trim());
		const events = lines.map((l) => JSON.parse(l) as Record<string, unknown>);
		const pktEvent = events.find((e) => e.type === "files.changed");

		expect(pktEvent).toBeDefined();
		if (!pktEvent) throw new Error("files.changed event should be present");

		const expectedKeys = ["type", "runId", "delegationId", "diffId", "changedFiles", "at"].sort();
		expect(Object.keys(pktEvent).sort()).toEqual(expectedKeys);
		expect(pktEvent.type).toBe("files.changed");
		expect(pktEvent.runId).toBe(RUN_ID);
		expect(pktEvent.delegationId).toBe("dlg-0001");
		expect(Array.isArray(pktEvent.changedFiles)).toBe(true);
	});

	it("{ok:false} on invalid runId (resolveRunDir guard)", () => {
		const obs = new Observe(ledgerRoot, nodeObserveDeps());
		const result = obs.capture({ repoRoot: repoDir, runId: "../evil", delegationId: "dlg-0001" });
		expect(result.ok).toBe(false);
	});

	it("LOW-3: writeFileSync throws → {ok:false} + P9 event already appended", () => {
		// LOW-3: artifact writes wrapped in try/catch (P4 tagged-union contract).
		// P9: files.changed event is appended BEFORE the write attempt — appendWasCalled=true
		// even when writeFileSync throws. Event serves as recovery marker.
		class WriteFailDeps implements ObserveDeps {
			appendWasCalled = false;
			constructor(private readonly real: ObserveDeps) {}
			execGit(args: string[], cwd: string) {
				return this.real.execGit(args, cwd);
			}
			writeFileSync(_path: string, _data: string): void {
				throw new Error("injected writeFileSync failure");
			}
			appendFileSync(path: string, data: string): void {
				this.appendWasCalled = true;
				this.real.appendFileSync(path, data);
			}
			mkdirSync(path: string, opts: { recursive: boolean }) {
				this.real.mkdirSync(path, opts);
			}
			existsSync(path: string) {
				return this.real.existsSync(path);
			}
			readdirSync(path: string) {
				return this.real.readdirSync(path);
			}
		}
		const writeFail = new WriteFailDeps(nodeObserveDeps());
		const obs = new Observe(ledgerRoot, writeFail);
		const result = obs.capture({ repoRoot: repoDir, runId: RUN_ID, delegationId: "dlg-0001" });
		// P4: returns {ok:false}, does NOT throw
		expect(result.ok).toBe(false);
		// P9 preserved: event was appended before the write that failed
		expect(writeFail.appendWasCalled).toBe(true);
	});
});
