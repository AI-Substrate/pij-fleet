// skills/flow-pair/test/context-pack-extract.test.ts
// T001: extractSection (6 tests) + T002: clusterLearnings (3 tests)
// All tests go RED against the stub (which throws "not implemented").

import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ContextPackCompiler } from "../lib/context-pack.js";
import { FLOW_PAIR_SKILL_ROOT } from "../lib/paths.js";

// ─── Shared fixture helpers ───────────────────────────────────────────────────

/** Write a file inside tmpRoot, creating parent dirs. Returns absolute path. */
function writeFixture(tmpRoot: string, relPath: string, content: string): string {
	const abs = join(tmpRoot, relPath);
	mkdirSync(join(abs, ".."), { recursive: true });
	writeFileSync(abs, content, "utf8");
	return abs;
}

// ─── T001: extractSection ─────────────────────────────────────────────────────

describe("ContextPackCompiler.extractSection — T001", () => {
	let tmpRoot: string;
	let compiler: ContextPackCompiler;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-extract-"));
		compiler = new ContextPackCompiler(tmpRoot, tmpRoot);
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("returns {ok:true} with correct section content when heading matches", () => {
		const planPath = writeFixture(
			tmpRoot,
			"plan.md",
			[
				"## Phase 1: Identity",
				"Identity content here.",
				"",
				"## Phase 2: Ledger",
				"Ledger content here.",
			].join("\n"),
		);
		const result = compiler.extractSection(planPath, "Phase 1: Identity");
		expect(result.ok).toBe(true);
		expect(result.content).toContain("Identity content here.");
		expect(result.content).not.toContain("Ledger content here.");
	});

	it("returns {ok:false} when section heading not found (mutation guard: remove early-return → RED)", () => {
		const planPath = writeFixture(tmpRoot, "plan.md", "## Phase 1: Identity\nContent.\n");
		const result = compiler.extractSection(planPath, "Phase 99: Nonexistent");
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/section not found/i);
	});

	it("returns {ok:false} when file does not exist (mutation guard: remove ENOENT catch → RED)", () => {
		const result = compiler.extractSection(join(tmpRoot, "no-such-file.md"), "Phase 1");
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/not found|ENOENT/i);
	});

	it("captures nested subsections (headings of lower level inside the section)", () => {
		const planPath = writeFixture(
			tmpRoot,
			"plan.md",
			[
				"## Phase 3: Compiler",
				"Intro line.",
				"",
				"### Sub-section A",
				"Sub-section content.",
				"",
				"## Phase 4: Next",
				"Should not be captured.",
			].join("\n"),
		);
		const result = compiler.extractSection(planPath, "Phase 3: Compiler");
		expect(result.ok).toBe(true);
		expect(result.content).toContain("Sub-section A");
		expect(result.content).toContain("Sub-section content.");
		expect(result.content).not.toContain("Phase 4");
	});

	it("stops at next same-level heading (boundary correctness)", () => {
		const planPath = writeFixture(
			tmpRoot,
			"plan.md",
			[
				"# Top",
				"",
				"## Phase 1: First",
				"First content.",
				"",
				"## Phase 2: Second",
				"Second content.",
			].join("\n"),
		);
		const result = compiler.extractSection(planPath, "Phase 1: First");
		expect(result.ok).toBe(true);
		expect(result.content).toContain("First content.");
		expect(result.content).not.toContain("Second content.");
	});

	it("prefix-colon match: finds heading by prefix (e.g. 'Phase 3' matches 'Phase 3: Compiler')", () => {
		const planPath = writeFixture(
			tmpRoot,
			"plan.md",
			[
				"## Phase 10: Ten",
				"Ten content.",
				"",
				"## Phase 3: Compiler",
				"Phase 3 content.",
				"",
				"## Phase 4: Next",
				"Not captured.",
			].join("\n"),
		);
		// 'Phase 3' must NOT match 'Phase 10' (substring would), must match 'Phase 3: Compiler'
		const result = compiler.extractSection(planPath, "Phase 3");
		expect(result.ok).toBe(true);
		expect(result.content).toContain("Phase 3 content.");
		expect(result.content).not.toContain("Ten content.");
	});
});

// ─── T002: clusterLearnings ───────────────────────────────────────────────────
// DL-003: learnings resolve from the SKILL root (4th ctor arg), never repoRoot.

describe("ContextPackCompiler.clusterLearnings — T002", () => {
	let tmpRoot: string;
	let repoRoot: string;
	let skillRoot: string;
	let compiler: ContextPackCompiler;

	beforeEach(async () => {
		tmpRoot = await mkdtemp(join(tmpdir(), "fp-cluster-"));
		repoRoot = join(tmpRoot, "repo");
		skillRoot = join(tmpRoot, "skill");
		mkdirSync(repoRoot, { recursive: true });
		mkdirSync(skillRoot, { recursive: true });
		compiler = new ContextPackCompiler(repoRoot, repoRoot, undefined, skillRoot);
	});

	afterEach(async () => {
		await rm(tmpRoot, { recursive: true, force: true });
	});

	it("returns {ok:true, learnings:[]} + searchedPath when cluster dir absent (graceful — honest miss)", () => {
		// No cluster dir created at all
		const result = compiler.clusterLearnings("implement-code");
		expect(result.ok).toBe(true);
		expect(result.learnings).toEqual([]);
		expect(result.searchedPath).toBe(
			join(skillRoot, "prompt-lab/clusters/implement-code/active.md"),
		);
	});

	it("returns {ok:true, learnings:[]} + searchedPath when cluster dir exists but active.md absent", () => {
		mkdirSync(join(skillRoot, "prompt-lab/clusters/implement-code"), {
			recursive: true,
		});
		const result = compiler.clusterLearnings("implement-code");
		expect(result.ok).toBe(true);
		expect(result.learnings).toEqual([]);
		expect(result.searchedPath).toContain("active.md");
	});

	it("returns one ClusterLearning entry with correct content when active.md present under skillRoot", () => {
		const clusterDir = join(skillRoot, "prompt-lab/clusters/implement-code");
		mkdirSync(clusterDir, { recursive: true });
		const activeContent = "# Learnings\nAlways TDD first.";
		writeFileSync(join(clusterDir, "active.md"), activeContent, "utf8");

		const result = compiler.clusterLearnings("implement-code");
		expect(result.ok).toBe(true);
		expect(result.learnings).toHaveLength(1);
		expect(result.learnings?.[0]?.cluster).toBe("implement-code");
		expect(result.learnings?.[0]?.content).toBe(activeContent);
		expect(result.learnings?.[0]?.sourcePath).toContain("active.md");
	});

	it("DL-003 root fix: repoRoot-joined layout is IGNORED — only skillRoot is searched (mutation guard: rejoin repoRoot → RED)", () => {
		// Old (buggy) layout under the CONSUMING repo — must not be picked up.
		const repoClusterDir = join(repoRoot, "skills/flow-pair/prompt-lab/clusters/implement-code");
		mkdirSync(repoClusterDir, { recursive: true });
		writeFileSync(join(repoClusterDir, "active.md"), "REPO-ROOT LEARNINGS — WRONG", "utf8");

		const result = compiler.clusterLearnings("implement-code");
		expect(result.ok).toBe(true);
		expect(result.learnings).toEqual([]);
		expect(result.searchedPath).toContain(skillRoot);
	});

	it("defaults skillRoot to the installed flow-pair skill root (absolute)", () => {
		const defaulted = new ContextPackCompiler(repoRoot, repoRoot);
		expect(isAbsolute(defaulted.skillRoot)).toBe(true);
		expect(defaulted.skillRoot).toBe(FLOW_PAIR_SKILL_ROOT);
		// The installed root really is the flow-pair skill dir (has prompt-lab/ + references/)
		expect(existsSync(join(defaulted.skillRoot, "prompt-lab"))).toBe(true);
		expect(existsSync(join(defaulted.skillRoot, "references", "templates"))).toBe(true);
	});
});
