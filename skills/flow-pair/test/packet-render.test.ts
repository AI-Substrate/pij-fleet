// skills/flow-pair/test/packet-render.test.ts
// T001: renderBody — required sections + content (6 tests)
// Fix 5: single-pass substitution (1 test)
// Fix 6: manifest validation (2 tests)
// T004: pointer-message format (2 tests)

import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ContextPackManifest } from "../lib/context-pack.js";
import { LedgerWriter, nodeLedgerDeps, PROMPTS_DIR } from "../lib/ledger.js";
import { nodePacketRendererDeps, PacketRenderer } from "../lib/packet.js";
import { FLOW_PAIR_SKILL_ROOT } from "../lib/paths.js";

// ─── Sample template with all required placeholders ───────────────────────────

const SAMPLE_TEMPLATE = `\
# Worker Packet — {{DELEGATION_ID}}

**Run**: {{RUN_ID}}
**Phase**: {{PHASE}}

## Mission

{{TASK_DESCRIPTION}}

## Repo Root

{{REPO_ROOT}}

## Forbidden Paths (NEVER edit these)

{{FORBIDDEN_PATHS}}

## Allowed Scope

{{ALLOWED_PATHS}}

## Context

### Phase Plan Section

{{PLAN_PHASE_CONTENT}}

### Tasks

{{TASKS_CONTENT}}

### Execution Log

{{EXEC_LOG_CONTENT}}

### Cluster Learnings

{{LEARNINGS_CONTENT}}

## Report Schema

\`\`\`json
{
  "delegationId": "{{DELEGATION_ID}}",
  "outcome": "COMPLETE | PARTIAL | BLOCKED",
  "summary": "1-3 sentences"
}
\`\`\`

## Stop Conditions

- Do NOT edit forbidden paths
- Send report via pij_send before stopping
`;

const FIXED_RUN_ID = "2026-06-17T00-00-00Z-p4render-xx";

function makeManifest(overrides: Partial<ContextPackManifest> = {}): ContextPackManifest {
	return {
		packId: "cp-0001",
		runId: FIXED_RUN_ID,
		delegationId: "dlg-0001",
		phase: "Phase 4: Test",
		cluster: "implement-code",
		entries: [
			{
				path: "/repo/plan.md",
				section: "Phase 4",
				content: "Phase 4 plan section content.",
				hash: "abcd1234",
				role: "plan-phase",
			},
			{
				path: "/repo/tasks.md",
				content: "Tasks content here.",
				hash: "efgh5678",
				role: "tasks",
			},
		],
		exclusions: [],
		allowedPaths: ["skills/flow-pair/"],
		forbiddenPaths: [".the-flow-state.json", "the-flow.json"],
		createdAt: "2026-06-17T00:00:00.000Z",
		...overrides,
	};
}

function scaffoldRun(ledgerRoot: string, runId: string): string {
	const runDir = join(ledgerRoot, "runs", runId);
	mkdirSync(join(runDir, PROMPTS_DIR), { recursive: true });
	mkdirSync(join(runDir, "prompt-trials"), { recursive: true });
	mkdirSync(join(runDir, "delegations"), { recursive: true });
	return runDir;
}

// T001: renderBody ─────────────────────────────────────────────────────────────

describe("PacketRenderer — renderBody (T001)", () => {
	let tmpRoot: string;
	let templateDir: string;
	let renderer: PacketRenderer;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "pkt-render-"));
		templateDir = join(tmpRoot, "templates");
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-implement.md"), SAMPLE_TEMPLATE, "utf8");
		const writer = new LedgerWriter(tmpRoot, nodeLedgerDeps());
		renderer = new PacketRenderer(tmpRoot, templateDir, writer, nodePacketRendererDeps());
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("body contains each forbidden path verbatim", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain(".the-flow-state.json");
		expect(result.body).toContain("the-flow.json");
	});

	it("body contains allowed paths", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain("skills/flow-pair/");
	});

	it("body contains plan-phase entry content", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain("Phase 4 plan section content.");
	});

	it("body contains tasks content when tasks entry is present", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain("Tasks content here.");
	});

	it("body contains stub text when no tasks entry is present", () => {
		const manifest = makeManifest({
			entries: [
				{
					path: "/repo/plan.md",
					section: "Phase 4",
					content: "Plan content.",
					hash: "abcd1234",
					role: "plan-phase",
				},
			],
		});
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain("(no tasks found)");
	});

	it("body contains report-schema section with delegationId field", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain("Report Schema");
		expect(result.body).toContain("delegationId");
	});
});

// Fix 5: single-pass substitution ─────────────────────────────────────────────

describe("PacketRenderer — renderBody single-pass (Fix 5)", () => {
	let tmpRoot: string;
	let templateDir: string;
	let renderer: PacketRenderer;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "pkt-singlepass-"));
		templateDir = join(tmpRoot, "templates");
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-implement.md"), SAMPLE_TEMPLATE, "utf8");
		const writer = new LedgerWriter(tmpRoot, nodeLedgerDeps());
		renderer = new PacketRenderer(tmpRoot, templateDir, writer, nodePacketRendererDeps());
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("injected tasks content containing a marker is NOT re-substituted (single-pass)", () => {
		// Multi-pass replace: "{{LEARNINGS_CONTENT}}" inside tasks body gets re-substituted.
		// Single-pass: the marker is left verbatim after the first (and only) scan.
		const poisonContent = "Tasks: check {{LEARNINGS_CONTENT}} for prior art";
		const manifest = makeManifest({
			entries: [
				{
					path: "/plan.md",
					section: "Phase 4",
					content: "Plan.",
					hash: "aa",
					role: "plan-phase",
				},
				{ path: "/tasks.md", content: poisonContent, hash: "bb", role: "tasks" },
			],
		});
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		// The literal marker must appear unchanged in the body (not re-substituted from within tasks)
		expect(result.body).toContain("{{LEARNINGS_CONTENT}}");
		// More precisely: the full injected string is verbatim (not re-substituted)
		expect(result.body).toContain("Tasks: check {{LEARNINGS_CONTENT}} for prior art");
		// The template's OWN {{LEARNINGS_CONTENT}} IS legitimately replaced with "(none)"
		// — we do NOT assert not.toContain("(none)") here.
	});
});

// Fix 6: manifest validation ───────────────────────────────────────────────────

describe("PacketRenderer — renderBody manifest validation (Fix 6)", () => {
	let tmpRoot: string;
	let templateDir: string;
	let renderer: PacketRenderer;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "pkt-guard-"));
		templateDir = join(tmpRoot, "templates");
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-implement.md"), SAMPLE_TEMPLATE, "utf8");
		const writer = new LedgerWriter(tmpRoot, nodeLedgerDeps());
		renderer = new PacketRenderer(tmpRoot, templateDir, writer, nodePacketRendererDeps());
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("{ok:false} when manifest has no plan-phase entry", () => {
		const manifest = makeManifest({
			entries: [{ path: "/tasks.md", content: "Tasks.", hash: "cc", role: "tasks" }],
		});
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		// Guard: if (!planEntry) return {ok:false}
		// Mutation `if (false)` -> renderBody succeeds -> test flips RED
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/plan-phase/);
	});

	it("{ok:false} when forbiddenPaths is empty", () => {
		const manifest = makeManifest({ forbiddenPaths: [] });
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/forbiddenPaths/);
	});
});

// DL-003: skill-root injection ─────────────────────────────────────────────────

describe("PacketRenderer — {{SKILL_ROOT}} injection (DL-003)", () => {
	const SKILL_ROOT_TEMPLATE = `${SAMPLE_TEMPLATE}\n## Protocol References\n\nSkill root: {{SKILL_ROOT}}\nProtocol: {{SKILL_ROOT}}/references/orchestrator-worker-protocol.md\n`;
	let tmpRoot: string;
	let templateDir: string;
	let renderer: PacketRenderer;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "pkt-skillroot-"));
		templateDir = join(tmpRoot, "templates");
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-implement.md"), SKILL_ROOT_TEMPLATE, "utf8");
		const writer = new LedgerWriter(tmpRoot, nodeLedgerDeps());
		renderer = new PacketRenderer(tmpRoot, templateDir, writer, nodePacketRendererDeps());
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("substitutes an explicit opts.skillRoot verbatim (mutation guard: drop SKILL_ROOT sub → RED)", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
			skillRoot: "/opt/agents/skills/flow-pair",
		});
		expect(result.ok).toBe(true);
		expect(result.body).toContain("Skill root: /opt/agents/skills/flow-pair");
		expect(result.body).toContain(
			"/opt/agents/skills/flow-pair/references/orchestrator-worker-protocol.md",
		);
		expect(result.body).not.toContain("{{SKILL_ROOT}}");
	});

	it("defaults to the installed flow-pair skill root when opts.skillRoot is omitted", () => {
		const manifest = makeManifest();
		const result = renderer.renderBody(manifest, {
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		// The default is the real install root — absolute, and cited references exist there.
		expect(isAbsolute(FLOW_PAIR_SKILL_ROOT)).toBe(true);
		expect(result.body).toContain(`Skill root: ${FLOW_PAIR_SKILL_ROOT}`);
		expect(
			existsSync(join(FLOW_PAIR_SKILL_ROOT, "references", "orchestrator-worker-protocol.md")),
		).toBe(true);
		expect(existsSync(join(FLOW_PAIR_SKILL_ROOT, "references", "review-rubrics.md"))).toBe(true);
	});

	it("the shipped worker-implement.md template carries {{SKILL_ROOT}} (drift guard)", () => {
		const shipped = readFileSync(
			join(FLOW_PAIR_SKILL_ROOT, "references", "templates", "worker-implement.md"),
			"utf8",
		);
		expect(shipped).toContain("{{SKILL_ROOT}}");
		// Repo-agnostic gates: the shipped worker template must not cite pij-repo just recipes.
		expect(shipped).not.toMatch(/just flow-pair-test|just typecheck|just lint/);
	});
});

// T004: pointer-message format ─────────────────────────────────────────────────

describe("PacketRenderer — pointer message format (T004)", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let templateDir: string;
	let renderer: PacketRenderer;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "pkt-ptr-"));
		ledgerRoot = join(tmpRoot, ".flow-pair");
		templateDir = join(tmpRoot, "templates");
		mkdirSync(templateDir, { recursive: true });
		writeFileSync(join(templateDir, "worker-implement.md"), SAMPLE_TEMPLATE, "utf8");
		scaffoldRun(ledgerRoot, FIXED_RUN_ID);
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("pointerMsg contains the delegationId", () => {
		const manifest = makeManifest();
		const result = renderer.writePacket({
			manifest,
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.packet?.pointerMsg).toContain("dlg-0001");
	});

	it("pointerMsg uses a relative path, not the absolute ledgerRoot", () => {
		const manifest = makeManifest();
		const result = renderer.writePacket({
			manifest,
			taskDescription: "Build it",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		const msg = result.packet?.pointerMsg ?? "";
		// Must not embed the absolute ledger path
		expect(msg).not.toContain(ledgerRoot);
		// Must use path.relative(repoRoot, packetPath)
		const expectedRel = relative(
			tmpRoot,
			join(ledgerRoot, "runs", FIXED_RUN_ID, PROMPTS_DIR, "dlg-0001.md"),
		);
		expect(msg).toContain(expectedRel);
	});
});
