// skills/flow-pair/test/packet-write.test.ts
// T002: writePacket — record-writing + pointer (7 tests)
// T003: P9 invariant in writePacket (3 tests)
// All RED against the stub; turn GREEN after T005 implementation.

import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ContextPackManifest } from "../lib/context-pack.js";
import {
	LedgerWriter,
	type LedgerWriter as LedgerWriterType,
	nodeLedgerDeps,
	PROMPTS_DIR,
	type PromptTrialRecord,
} from "../lib/ledger.js";
import { nodePacketRendererDeps, PacketRenderer, type PacketRendererDeps } from "../lib/packet.js";

// ─── Constants ────────────────────────────────────────────────────────────────

const FIXED_RUN_ID = "2026-06-17T00-00-00Z-p4write-xx";

const SAMPLE_TEMPLATE = `\
# Worker Packet — {{DELEGATION_ID}}
**Run**: {{RUN_ID}}
## Mission
{{TASK_DESCRIPTION}}
## Repo Root
{{REPO_ROOT}}
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

// ─── Helpers ──────────────────────────────────────────────────────────────────

function scaffoldRun(ledgerRoot: string, runId: string): string {
	const runDir = join(ledgerRoot, "runs", runId);
	mkdirSync(join(runDir, PROMPTS_DIR), { recursive: true });
	mkdirSync(join(runDir, "prompt-trials"), { recursive: true });
	mkdirSync(join(runDir, "delegations"), { recursive: true });
	return runDir;
}

function makeManifest(overrides: Partial<ContextPackManifest> = {}): ContextPackManifest {
	return {
		packId: "cp-0001",
		runId: FIXED_RUN_ID,
		delegationId: "dlg-0001",
		phase: "Phase 4: Test",
		cluster: "implement-code",
		entries: [
			{
				path: "/plan.md",
				section: "Phase 4",
				content: "Plan content.",
				hash: "aabb",
				role: "plan-phase",
			},
			{ path: "/tasks.md", content: "Tasks here.", hash: "ccdd", role: "tasks" },
		],
		exclusions: [],
		allowedPaths: ["skills/flow-pair/"],
		forbiddenPaths: [".the-flow-state.json"],
		createdAt: "2026-06-17T00:00:00.000Z",
		...overrides,
	};
}

/** FakeLedgerWriter — always returns {ok:false} from writePromptTrial (F4 injection). */
function makeFakeLedgerWriter(): LedgerWriterType {
	return {
		writePromptTrial: (): { ok: false; error: string } => ({
			ok: false,
			error: "injected trial failure",
		}),
	} as unknown as LedgerWriterType;
}

/** TrackingDeps — wraps real fs, records every appendFileSync + writeFileSync call. */
function makeTrackingDeps(): PacketRendererDeps & { callLog: string[] } {
	const real = nodePacketRendererDeps();
	const callLog: string[] = [];
	return {
		callLog,
		readFileSync: (path, enc) => real.readFileSync(path, enc),
		existsSync: (path) => real.existsSync(path),
		appendFileSync: (path, data) => {
			callLog.push(`appendFileSync:${path}`);
			real.appendFileSync(path, data);
		},
		writeFileSync: (path, data) => {
			callLog.push(`writeFileSync:${path}`);
			real.writeFileSync(path, data);
		},
	};
}

/**
 * FailAppendDeps — wraps real fs for reads/exists, but appendFileSync always throws.
 * Tracks whether writeFileSync was called (must be false if P9 guard works).
 * Uses real readFileSync so the template can be loaded (Phase 3 lesson: deps that fail
 * BEFORE the guard line give a vacuous green — only override the exact failing call).
 */
function makeFailAppendDeps(): PacketRendererDeps & { writeWasCalled: boolean } {
	const real = nodePacketRendererDeps();
	let writeWasCalled = false;
	return {
		get writeWasCalled() {
			return writeWasCalled;
		},
		readFileSync: (path, enc) => real.readFileSync(path, enc),
		existsSync: (path) => real.existsSync(path),
		appendFileSync: () => {
			throw new Error("disk full — injected failure");
		},
		writeFileSync: () => {
			writeWasCalled = true;
		},
	};
}

// ─── Shared fixture ───────────────────────────────────────────────────────────

let tmpRoot: string;
let ledgerRoot: string;
let templateDir: string;
let runDir: string;

beforeEach(() => {
	tmpRoot = mkdtempSync(join(tmpdir(), "pkt-write-"));
	ledgerRoot = join(tmpRoot, ".flow-pair");
	templateDir = join(tmpRoot, "templates");
	mkdirSync(templateDir, { recursive: true });
	writeFileSync(join(templateDir, "worker-implement.md"), SAMPLE_TEMPLATE, "utf8");
	runDir = scaffoldRun(ledgerRoot, FIXED_RUN_ID);
});

afterEach(() => {
	rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
});

// ─── Fix 4: delegationId validation ─────────────────────────────────────────

describe("PacketRenderer — delegationId validation (Fix 4)", () => {
	it("{ok:false} for path-traversal delegationId (../evil)", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest({ delegationId: "../evil" }),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		// Guard: if (!DLG_ID_RE.test(manifest.delegationId))
		// Mutation `if (false)` → result.ok becomes true → test flips RED
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/invalid delegationId/);
	});

	it("{ok:false} for newline-injection delegationId (dlg-0001\\nextra)", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest({ delegationId: "dlg-0001\nextra" }),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/invalid delegationId/);
	});

	it("{ok:false} for bracket-injection delegationId (dlg-0001] injected)", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest({ delegationId: "dlg-0001] injected" }),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(false);
		expect(result.error).toMatch(/invalid delegationId/);
	});
});

describe("PacketRenderer — writePacket record-writing (T002)", () => {
	it("trial record is written to prompt-trials/<trialId>.json", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build Phase 4",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		const trialsDir = join(runDir, "prompt-trials");
		const trialFiles = existsSync(trialsDir)
			? readdirSync(trialsDir).filter((f) => f.endsWith(".json"))
			: [];
		expect(trialFiles.length).toBe(1);
		const trial = JSON.parse(
			readFileSync(join(trialsDir, trialFiles[0]), "utf8"),
		) as PromptTrialRecord;
		expect(trial.delegationId).toBe("dlg-0001");
		expect(trial.runId).toBe(FIXED_RUN_ID);
	});

	it("packet body is written to prompts/<delegationId>.md", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build Phase 4",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		const packetPath = join(runDir, PROMPTS_DIR, "dlg-0001.md");
		expect(existsSync(packetPath)).toBe(true);
		const body = readFileSync(packetPath, "utf8");
		expect(body.length).toBeGreaterThan(0);
	});

	it("packetPath in result matches expected join(runDir, PROMPTS_DIR, delegationId.md)", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build Phase 4",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		const expected = join(runDir, PROMPTS_DIR, "dlg-0001.md");
		expect(result.packet?.packetPath).toBe(expected);
	});

	it("delegationId in packet matches manifest.delegationId", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build Phase 4",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.packet?.delegationId).toBe("dlg-0001");
	});

	it("promptHash is an 8-char hex string of the body", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build Phase 4",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);
		expect(result.packet?.promptHash).toMatch(/^[0-9a-f]{8}$/);
	});

	it("{ok:false} on invalid runId (path-traversal guard)", () => {
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, nodePacketRendererDeps());
		const result = renderer.writePacket({
			manifest: makeManifest({ runId: "../traversal" }),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(false);
		expect(result.packet).toBeUndefined();
	});

	it("{ok:false} when writePromptTrial fails (FakeLedgerWriter — F4)", () => {
		// FakeLedgerWriter injected via constructor (F4: writePromptTrial is a LedgerWriter
		// method — a PacketRendererDeps override cannot reach it)
		const fakeWriter = makeFakeLedgerWriter();
		const renderer = new PacketRenderer(
			ledgerRoot,
			templateDir,
			fakeWriter,
			nodePacketRendererDeps(),
		);
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		// Guard: if (!trialResult.ok) return {ok:false}
		// Mutation `if (false)` would make result.ok === true → test flips RED
		expect(result.ok).toBe(false);
		expect(result.packet).toBeUndefined();
	});
});

// ─── T003: P9 invariant ───────────────────────────────────────────────────────

describe("PacketRenderer — P9 invariant (T003)", () => {
	it("P9: appendFileSync (events.jsonl) is called before writeFileSync (packet file)", () => {
		const deps = makeTrackingDeps();
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, deps);
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(true);

		// Find the events.jsonl append and the prompts/ write in the call log
		const appendIdx = deps.callLog.findIndex(
			(c) => c.startsWith("appendFileSync:") && c.includes("events.jsonl"),
		);
		const writeIdx = deps.callLog.findIndex(
			(c) => c.startsWith("writeFileSync:") && c.includes(PROMPTS_DIR),
		);
		expect(appendIdx).toBeGreaterThanOrEqual(0); // event was appended
		expect(writeIdx).toBeGreaterThanOrEqual(0); // packet was written
		// P9: event append MUST precede packet write
		expect(appendIdx).toBeLessThan(writeIdx);
	});

	it("P9 failure injection: appendFileSync throws → {ok:false}, packet file NOT written", () => {
		// FailDeps wraps real fs — only appendFileSync throws (Phase 3 lesson:
		// deps that fail BEFORE the guard give a vacuous green; override only the exact call).
		// readFileSync wraps real so the template is loaded successfully.
		const deps = makeFailAppendDeps();
		const writer = new LedgerWriter(ledgerRoot, nodeLedgerDeps());
		const renderer = new PacketRenderer(ledgerRoot, templateDir, writer, deps);
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		// Guard: if (!ev.ok) return {ok:false, error}
		// Mutation `if (false)` → writeFileSync IS called → both assertions below flip RED
		expect(result.ok).toBe(false); // ← flips under mutation (result.ok becomes true)
		expect(deps.writeWasCalled).toBe(false); // ← flips under mutation (writeWasCalled becomes true)
	});

	it("trial failure: writePacket returns {ok:false}; packet file was already written", () => {
		// FakeLedgerWriter returns {ok:false} from writePromptTrial.
		// Packet file is written first (step 5), THEN writePromptTrial fails (step 6).
		// Guard: if (!trialResult.ok) return {ok:false}
		// Mutation `if (false)` → result.ok becomes true → test flips RED
		const fakeWriter = makeFakeLedgerWriter();
		const renderer = new PacketRenderer(
			ledgerRoot,
			templateDir,
			fakeWriter,
			nodePacketRendererDeps(),
		);
		const result = renderer.writePacket({
			manifest: makeManifest(),
			taskDescription: "Build",
			repoRoot: tmpRoot,
		});
		expect(result.ok).toBe(false); // ← flips under mutation
		// Packet file exists — packet.written event + writeFileSync committed before trial failed
		const packetPath = join(runDir, PROMPTS_DIR, "dlg-0001.md");
		expect(existsSync(packetPath)).toBe(true);
	});
});
