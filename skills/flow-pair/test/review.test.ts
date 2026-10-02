// skills/flow-pair/test/review.test.ts
// Phase 6: Review + fix loop — TDD tests.
// T001: AC-05 guard (missing execution.log.md → FIX_REQUIRED)
// T002: AC-06 scope (fix packet allowedFiles = exactly finding files)
// T003: P9 ordering invariant

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ReviewFinding } from "../lib/ledger.js";
import { FLOW_PAIR_SKILL_ROOT } from "../lib/paths.js";
import type { ReviewDeps } from "../lib/review.js";
import { FINDING_KIND, nodeReviewDeps, Review, VERDICT } from "../lib/review.js";

// ─── Shared scaffold helper ───────────────────────────────────────────────────

/** Create a real run dir so appendLedgerEvent can write to events.jsonl (anti-vacuous trap). */
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
	// events.jsonl must exist so appendFileSync (append mode) can write to it
	writeFileSync(join(runDir, "events.jsonl"), "");
	return runDir;
}

/** Minimal worker-fix.md template for T002 fixture. */
const MINIMAL_TEMPLATE =
	"# Fix {{FIX_PACKET_ID}}\nDelegation: {{DELEGATION_ID}}\nReview: {{REVIEW_ID}}\nRun: {{RUN_ID}}\nAllowed:\n{{ALLOWED_FILES_LIST}}\nFindings:\n{{FINDINGS_SUMMARY}}\n";

// ─── TrackingDeps: records callLog for ordering assertions ────────────────────

class TrackingDeps implements ReviewDeps {
	callLog: Array<{ method: string; path: string }> = [];

	get appendWasCalled(): boolean {
		return this.callLog.some((e) => e.method === "appendFileSync");
	}
	get writeWasCalled(): boolean {
		return this.callLog.some((e) => e.method === "writeFileSync");
	}

	constructor(private readonly real: ReviewDeps) {}

	existsSync(path: string) {
		return this.real.existsSync(path);
	}
	readFileSync(path: string, enc: "utf8") {
		return this.real.readFileSync(path, enc);
	}
	readdirSync(path: string) {
		return this.real.readdirSync(path);
	}
	mkdirSync(path: string, opts: { recursive: boolean }) {
		this.real.mkdirSync(path, opts);
	}
	writeFileSync(path: string, data: string): void {
		this.callLog.push({ method: "writeFileSync", path });
		this.real.writeFileSync(path, data);
	}
	appendFileSync(path: string, data: string): void {
		this.callLog.push({ method: "appendFileSync", path });
		this.real.appendFileSync(path, data);
	}
}

// ─── FailDeps: appendFileSync throws so P9 guard fires; writeFileSync tracks ──

class FailAppendDeps implements ReviewDeps {
	appendWasCalled = false;
	writeWasCalled = false;

	constructor(private readonly real: ReviewDeps) {}

	existsSync(path: string) {
		return this.real.existsSync(path);
	}
	readFileSync(path: string, enc: "utf8") {
		return this.real.readFileSync(path, enc);
	}
	readdirSync(path: string) {
		return this.real.readdirSync(path);
	}
	mkdirSync(path: string, opts: { recursive: boolean }) {
		this.real.mkdirSync(path, opts);
	}
	appendFileSync(_path: string, _data: string): void {
		this.appendWasCalled = true;
		throw new Error("injected appendFileSync failure");
	}
	writeFileSync(_path: string, _data: string): void {
		// Track but don't write — this should never be called when P9 guard fires
		this.writeWasCalled = true;
	}
}

// ─── WriteFail: writeFileSync throws for P4 test ─────────────────────────────

class WriteFailDeps implements ReviewDeps {
	appendWasCalled = false;

	constructor(private readonly real: ReviewDeps) {}

	existsSync(path: string) {
		return this.real.existsSync(path);
	}
	readFileSync(path: string, enc: "utf8") {
		return this.real.readFileSync(path, enc);
	}
	readdirSync(path: string) {
		return this.real.readdirSync(path);
	}
	mkdirSync(path: string, opts: { recursive: boolean }) {
		this.real.mkdirSync(path, opts);
	}
	appendFileSync(path: string, data: string): void {
		this.appendWasCalled = true;
		this.real.appendFileSync(path, data);
	}
	writeFileSync(_path: string, _data: string): void {
		throw new Error("injected writeFileSync failure");
	}
}

// ─── T001: AC-05 guard ────────────────────────────────────────────────────────

describe("Review — evaluate() AC-05 (T001)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let repoRoot: string;
	let phaseDir: string;
	const RUN_ID = "2026-06-23T00-00-00Z-rev-t001-xxx";

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "review-t001-"));
		ledgerRoot = join(tmpRoot, "ledger");
		repoRoot = join(tmpRoot, "repo");
		phaseDir = join(repoRoot, "docs", "plans", "phase-6");
		scaffoldRunDir(ledgerRoot, RUN_ID);
		mkdirSync(phaseDir, { recursive: true });
		// execution.log.md is ABSENT — see test 3 for the presence branch
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("missing execution.log.md → verdict is FIX_REQUIRED [AC-05 load-bearing]", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.evaluate({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			phaseDir,
			repoRoot,
		});
		// evaluate() itself succeeded
		expect(result.ok).toBe(true);
		// AC-05 load-bearing: if guard is deleted (if(!logExists)→if(false)) this flips RED
		expect(result.verdict).toBe(VERDICT.FIX_REQUIRED);
		// finding must be present with correct dimension
		const finding = result.findings?.find((f) => f.dimension === FINDING_KIND.ARTIFACT_CONTRACT);
		expect(finding).toBeDefined();
		expect(finding?.message).toMatch(/execution\.log\.md/);
		// file must be repo-relative (not absolute)
		expect(finding?.file).toBeDefined();
		expect(finding?.file?.startsWith("/")).toBe(false);
	});

	it("missing execution.log.md → review record written as reviews/rev-0001.json", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		rev.evaluate({ runId: RUN_ID, delegationId: "dlg-0001", phaseDir, repoRoot });
		const runDir = join(ledgerRoot, "runs", RUN_ID);
		const recPath = join(runDir, "reviews", "rev-0001.json");
		expect(() => readFileSync(recPath, "utf8")).not.toThrow();
		const rec = JSON.parse(readFileSync(recPath, "utf8")) as Record<string, unknown>;
		expect(rec.verdict).toBe(VERDICT.FIX_REQUIRED);
		expect(rec.reviewId).toBe("rev-0001");
	});

	it("present execution.log.md + zero findings → REFUSES to mint a verdict (verdict law: APPROVE is never a default)", () => {
		// Write the artifact so the deterministic sweep finds nothing — the
		// dogfood case where a default APPROVE shadowed a reviewer's real
		// FIX_REQUIRED (s1 stream, rev-0001 stub).
		writeFileSync(join(phaseDir, "execution.log.md"), "## log\nDone.\n");
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.evaluate({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			phaseDir,
			repoRoot,
		});
		expect(result.ok).toBe(false);
		expect(result.verdict).toBeUndefined();
		expect(result.error).toContain("no findings to review");

		// Nothing recorded: no review file, no verdict minted anywhere.
		const recPath = join(ledgerRoot, "runs", RUN_ID, "reviews", "rev-0001.json");
		expect(() => readFileSync(recPath, "utf8")).toThrow();
	});

	it("P9: review.recorded event appended before reviews/<id>.json written", () => {
		const tracking = new TrackingDeps(nodeReviewDeps());
		const rev = new Review(ledgerRoot, tracking);
		const result = rev.evaluate({ runId: RUN_ID, delegationId: "dlg-0001", phaseDir, repoRoot });
		expect(result.ok).toBe(true);

		// Find the positions in callLog
		const runDir = join(ledgerRoot, "runs", RUN_ID);
		const appendIdx = tracking.callLog.findIndex(
			(e) => e.method === "appendFileSync" && e.path.includes("events.jsonl"),
		);
		const writeIdx = tracking.callLog.findIndex(
			(e) => e.method === "writeFileSync" && e.path.includes("reviews"),
		);
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThanOrEqual(0);
		expect(appendIdx).toBeLessThan(writeIdx); // appendFileSync before writeFileSync
		void runDir; // suppress unused warning
	});

	it("P4: writeFileSync throws → {ok:false}, no uncaught exception", () => {
		const writeFail = new WriteFailDeps(nodeReviewDeps());
		const rev = new Review(ledgerRoot, writeFail);
		// Should NOT throw — must return {ok:false}
		expect(() => {
			const result = rev.evaluate({ runId: RUN_ID, delegationId: "dlg-0001", phaseDir, repoRoot });
			expect(result.ok).toBe(false);
			// P9 preserved: event was appended before the write that failed
			expect(writeFail.appendWasCalled).toBe(true);
		}).not.toThrow();
	});

	it("P9 guard: FailAppendDeps appendFileSync throws → {ok:false} + review NOT written", () => {
		const failDeps = new FailAppendDeps(nodeReviewDeps());
		const rev = new Review(ledgerRoot, failDeps);
		const result = rev.evaluate({ runId: RUN_ID, delegationId: "dlg-0001", phaseDir, repoRoot });
		// P4: returns {ok:false}, does NOT throw
		expect(result.ok).toBe(false);
		// P9 load-bearing: writeFileSync (reviews/<id>.json) was NOT called because P9 guard fired
		// Mutation s/if (!ev.ok)/if (false)/ → writeWasCalled becomes true → this assertion flips RED
		expect(failDeps.writeWasCalled).toBe(false);
	});

	it("invalid runId → {ok:false, error}", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.evaluate({ runId: "../evil", delegationId: "dlg-0001", phaseDir, repoRoot });
		expect(result.ok).toBe(false);
		expect(result.error).toBeDefined();
	});
});

// ─── T002: AC-06 fix packet scope ────────────────────────────────────────────

describe("Review — generateFixPacket() AC-06 (T002)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let repoRoot: string;
	let templateDir: string;
	const RUN_ID = "2026-06-23T00-00-00Z-rev-t002-xxx";

	// 3 findings: 2 WITH file (the allowed scope), 1 WITHOUT (must be excluded)
	const FINDINGS: ReviewFinding[] = [
		{
			dimension: FINDING_KIND.TEST_QUALITY,
			severity: "high",
			message: "Missing mutation gate",
			file: "lib/review.ts",
		},
		{
			dimension: FINDING_KIND.ARTIFACT_CONTRACT,
			severity: "critical",
			message: "Missing execution.log.md",
			file: "docs/plans/phase-6/execution.log.md",
		},
		{
			dimension: FINDING_KIND.SCOPE,
			severity: "low",
			message: "Info note — no file association",
			// no file field → must NOT appear in allowedFiles
		},
	];

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "review-t002-"));
		ledgerRoot = join(tmpRoot, "ledger");
		repoRoot = join(tmpRoot, "repo");
		templateDir = join(tmpRoot, "templates");
		scaffoldRunDir(ledgerRoot, RUN_ID);
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-fix.md"), MINIMAL_TEMPLATE);
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("allowedFiles length is exactly 2 (findings with file only) [AC-06 load-bearing]", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);
		// AC-06 load-bearing: if extractAllowedFiles(opts.findings)→[] this flips RED (0 ≠ 2)
		expect(result.packet?.allowedFiles.length).toBe(2);
		expect(result.packet?.allowedFiles).toContain("lib/review.ts");
		expect(result.packet?.allowedFiles).toContain("docs/plans/phase-6/execution.log.md");
	});

	it("allowedFiles excludes files not in findings (non-vacuous scope probe)", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);
		// Non-vacuous: "lib/other.ts" is not in any finding.file → must not appear in allowedFiles
		// Mutation s/extractAllowedFiles(opts.findings)/[other array]/ → this assertion AND length flip RED
		expect(result.packet?.allowedFiles).not.toContain("lib/other.ts");
		// Finding without file field must not appear (undefined/null stripped)
		expect(result.packet?.allowedFiles).not.toContain(undefined);
	});

	it("allowedFiles deduplicates: two findings with same file → path appears once", () => {
		const dupFindings: ReviewFinding[] = [
			{
				dimension: FINDING_KIND.TEST_QUALITY,
				severity: "high",
				message: "A",
				file: "lib/review.ts",
			},
			{ dimension: FINDING_KIND.SCOPE, severity: "medium", message: "B", file: "lib/review.ts" },
		];
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: dupFindings,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.packet?.allowedFiles.length).toBe(1);
		expect(result.packet?.allowedFiles[0]).toBe("lib/review.ts");
	});

	it("allowedFiles preserves valid repo-relative files and dedupes exactly", () => {
		const findings: ReviewFinding[] = [
			{ dimension: FINDING_KIND.TEST_QUALITY, severity: "high", message: "A", file: "lib/a.ts" },
			{ dimension: FINDING_KIND.SCOPE, severity: "medium", message: "B", file: "docs/b.md" },
			{ dimension: FINDING_KIND.REGRESSION, severity: "low", message: "C", file: "lib/a.ts" },
		];
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.packet?.allowedFiles).toEqual(["lib/a.ts", "docs/b.md"]);
	});

	it.each([
		"",
		".",
		"./x",
		"/abs",
		"../x",
	])("unsafe finding.file %j → {ok:false} before writing fix packet", (file) => {
		const findings: ReviewFinding[] = [
			{
				dimension: FINDING_KIND.SCOPE,
				severity: "high",
				message: "Unsafe file path from review record",
				file,
			},
		];
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/finding\.file/);
	});

	it("P9: fix_packet.written event appended before fix-packets/<id>.md written", () => {
		const tracking = new TrackingDeps(nodeReviewDeps());
		const rev = new Review(ledgerRoot, tracking);
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);

		const appendIdx = tracking.callLog.findIndex(
			(e) => e.method === "appendFileSync" && e.path.includes("events.jsonl"),
		);
		const writeIdx = tracking.callLog.findIndex(
			(e) => e.method === "writeFileSync" && e.path.includes("fix-packets"),
		);
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(writeIdx).toBeGreaterThanOrEqual(0);
		expect(appendIdx).toBeLessThan(writeIdx);
	});

	it("P9 guard: FailAppendDeps appendFileSync throws → {ok:false} + fix files NOT written", () => {
		const failDeps = new FailAppendDeps(nodeReviewDeps());
		const rev = new Review(ledgerRoot, failDeps);
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(false);
		// P9 load-bearing: writeFileSync (fix-NNNN.md/.json) was NOT called
		// Mutation s/if (!ev.ok)/if (false)/ → writeWasCalled becomes true → flips RED
		expect(failDeps.writeWasCalled).toBe(false);
	});

	it("fix packet .md contains delegationId and reviewId from template substitution", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);
		if (!result.packet) throw new Error("expected fix packet");
		const mdContent = readFileSync(result.packet.fixPacketPath, "utf8");
		expect(mdContent).toContain("dlg-0001");
		expect(mdContent).toContain("rev-0001");
	});

	it("DL-003: {{SKILL_ROOT}} in worker-fix.md renders to an absolute skill root (explicit + default)", () => {
		writeFileSync(
			join(templateDir, "worker-fix.md"),
			`${MINIMAL_TEMPLATE}Protocol: {{SKILL_ROOT}}/references/orchestrator-worker-protocol.md\n`,
		);
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		// Explicit skillRoot is substituted verbatim
		const explicit = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
			skillRoot: "/opt/agents/skills/flow-pair",
		});
		expect(explicit.ok).toBe(true);
		if (!explicit.packet) throw new Error("expected fix packet");
		const explicitMd = readFileSync(explicit.packet.fixPacketPath, "utf8");
		expect(explicitMd).toContain(
			"Protocol: /opt/agents/skills/flow-pair/references/orchestrator-worker-protocol.md",
		);
		expect(explicitMd).not.toContain("{{SKILL_ROOT}}");
		// Omitted → defaults to the installed flow-pair skill root
		const defaulted = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0002",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(defaulted.ok).toBe(true);
		if (!defaulted.packet) throw new Error("expected fix packet");
		const defaultedMd = readFileSync(defaulted.packet.fixPacketPath, "utf8");
		expect(defaultedMd).toContain(`Protocol: ${FLOW_PAIR_SKILL_ROOT}/references/`);
		expect(defaultedMd).not.toContain("{{SKILL_ROOT}}");
	});

	it("pointerMsg format: [flow-pair fix-0001] Fix packet at: ...", () => {
		const rev = new Review(ledgerRoot, nodeReviewDeps());
		const result = rev.generateFixPacket({
			runId: RUN_ID,
			delegationId: "dlg-0001",
			reviewId: "rev-0001",
			findings: FINDINGS,
			templateDir,
			repoRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.packet?.pointerMsg).toMatch(/^\[flow-pair fix-0001\] Fix packet at: /);
	});
});
