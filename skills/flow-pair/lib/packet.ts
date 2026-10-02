// skills/flow-pair/lib/packet.ts
// Phase 4: Worker-packet renderer.
// P2: zero @earendil-works/* imports | P3: inject via constructor | P7: .js ESM imports
// P5: PROMPTS_DIR imported from ./ledger.js (single source — not redefined here)

import { createHash } from "node:crypto";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative } from "node:path";
import type { ContextPackManifest } from "./context-pack.js";
import type { PromptTrialRecord } from "./ledger.js";
import { appendLedgerEvent, PROMPTS_DIR } from "./ledger.js";
import { FLOW_PAIR_SKILL_ROOT, resolveRunDir } from "./paths.js";

// ─── Constants (P5) ──────────────────────────────────────────────────────────

export const PACKET_TEMPLATE_REF = "worker-implement@v1" as const;

/** Validation pattern for delegation IDs — must match dlg-NNNN. */
const DLG_ID_RE = /^dlg-\d{4}$/;

// ─── Types ────────────────────────────────────────────────────────────────────

export interface PacketRendererDeps {
	readFileSync(path: string, enc: "utf8"): string;
	writeFileSync(path: string, data: string): void;
	appendFileSync(path: string, data: string): void;
	/** Used transitively via appendLedgerEvent's runDir guard. */
	existsSync(path: string): boolean;
}

export interface WorkerPacket {
	delegationId: string;
	runId: string;
	packetPath: string;
	body: string;
	pointerMsg: string;
	templateRef: string;
	promptHash: string;
}

export interface WritePacketOpts {
	manifest: ContextPackManifest;
	taskDescription: string;
	repoRoot: string;
	templateRef?: string;
	/**
	 * Absolute root of the installed flow-pair skill, injected into the packet as
	 * {{SKILL_ROOT}} so cited references resolve in ANY consuming repo (DL-003).
	 * Defaults to the root this module is installed under.
	 */
	skillRoot?: string;
}

export interface RenderOpts {
	taskDescription: string;
	repoRoot: string;
	templateRef?: string;
	/** See WritePacketOpts.skillRoot — {{SKILL_ROOT}} substitution source (DL-003). */
	skillRoot?: string;
}

/** Minimal interface for PacketRenderer's writer dependency — only what writePacket calls. */
interface PacketLedgerWriter {
	writePromptTrial(
		runId: string,
		delegationId: string,
		opts: { templateRef: string; promptHash: string },
	): { ok: boolean; trial?: PromptTrialRecord; error?: string };
}

// ─── Production deps binding ──────────────────────────────────────────────────

export function nodePacketRendererDeps(): PacketRendererDeps {
	return {
		readFileSync: (path, enc) => readFileSync(path, enc),
		writeFileSync: (path, data) => writeFileSync(path, data, "utf8"),
		appendFileSync: (path, data) => appendFileSync(path, data, "utf8"),
		existsSync: (path) => existsSync(path),
	};
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

function sha256Slice8(data: string): string {
	return createHash("sha256").update(data).digest("hex").slice(0, 8);
}

function buildPointerMsg(delegationId: string, packetPath: string, repoRoot: string): string {
	const rel = relative(repoRoot, packetPath);
	return `[flow-pair ${delegationId}] Packet at: ${rel}`;
}

// ─── PacketRenderer ───────────────────────────────────────────────────────────

export class PacketRenderer {
	constructor(
		readonly ledgerRoot: string,
		readonly templateDir: string,
		private readonly writer: PacketLedgerWriter,
		private readonly deps: PacketRendererDeps = nodePacketRendererDeps(),
	) {}

	/**
	 * Render the worker-packet body from a manifest + options.
	 * Loads `worker-implement.md` from templateDir and replaces {{PLACEHOLDER}} markers
	 * in a single pass (Fix 5) to prevent re-substitution of injected content.
	 *
	 * Fix 6 guards (both load-bearing):
	 *   - plan-phase entry required (provides phase spec for worker)
	 *   - forbiddenPaths non-empty (absence would silently omit flow-state guard)
	 */
	renderBody(
		manifest: ContextPackManifest,
		opts: RenderOpts,
	): { ok: boolean; body?: string; error?: string } {
		try {
			// Load template
			const templatePath = join(this.templateDir, "worker-implement.md");
			let template: string;
			try {
				template = this.deps.readFileSync(templatePath, "utf8");
			} catch {
				return { ok: false, error: `template not found: ${templatePath}` };
			}

			// Fix 6: plan-phase entry is load-bearing (provides phase spec for the worker)
			const planEntry = manifest.entries.find((e) => e.role === "plan-phase");
			if (!planEntry) {
				return {
					ok: false,
					error: "manifest has no plan-phase entry (required for rendering)",
				};
			}

			// Fix 6: forbiddenPaths guard is load-bearing (absent = flow-state files unprotected)
			if (manifest.forbiddenPaths.length === 0) {
				return {
					ok: false,
					error: "manifest forbiddenPaths is empty — flow-state guard would be absent",
				};
			}

			const tasksEntry = manifest.entries.find((e) => e.role === "tasks");
			const execLogEntry = manifest.entries.find((e) => e.role === "execution-log");
			const learningEntries = manifest.entries.filter((e) => e.role === "learning");

			const tasksContent = tasksEntry?.content ?? "(no tasks found)";
			const execLogContent = execLogEntry?.content ?? "(not yet created)";
			const learningsContent =
				learningEntries.length > 0
					? learningEntries.map((e) => e.content).join("\n---\n")
					: "(none)";

			const forbiddenPaths = manifest.forbiddenPaths.map((p) => `- ${p}`).join("\n");
			const allowedPaths = manifest.allowedPaths.map((p) => `- ${p}`).join("\n");

			// Fix 5: single-pass substitution — prevents a marker name inside injected content
			// (e.g. "{{LEARNINGS_CONTENT}}" in the tasks body) from being re-substituted.
			const subs: Record<string, string> = {
				DELEGATION_ID: manifest.delegationId,
				RUN_ID: manifest.runId,
				PHASE: manifest.phase,
				TASK_DESCRIPTION: opts.taskDescription,
				REPO_ROOT: opts.repoRoot,
				// DL-003: absolute install root of the flow-pair skill — packet citations
				// (protocol, rubrics) resolve against this in any consuming repo.
				SKILL_ROOT: opts.skillRoot ?? FLOW_PAIR_SKILL_ROOT,
				FORBIDDEN_PATHS: forbiddenPaths,
				ALLOWED_PATHS: allowedPaths,
				PLAN_PHASE_CONTENT: planEntry.content,
				TASKS_CONTENT: tasksContent,
				EXEC_LOG_CONTENT: execLogContent,
				LEARNINGS_CONTENT: learningsContent,
			};
			const body = template.replace(
				/\{\{([A-Z_]+)\}\}/g,
				(match, key: string) => subs[key] ?? match,
			);

			return { ok: true, body };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}

	/**
	 * Render + write the worker packet, record a prompt trial, return WorkerPacket.
	 *
	 * Fix 4: delegationId is validated against /^dlg-\d{4}$/ before any path construction
	 * to prevent path-traversal (`../evil`), newline-injection (`dlg-0001\nextra`), and
	 * bracket-injection (`dlg-0001] injected`).
	 *
	 * P9 order (persist-before-mutate):
	 *   1. Compute body + packetPath (no side effects)
	 *   2. appendLedgerEvent(packet.written)   ← BEFORE writeFileSync
	 *   3. writeFileSync(prompts/<delegationId>.md)
	 *   4. writePromptTrial(...)               ← internally P9-correct (Phase 2)
	 */
	writePacket(opts: WritePacketOpts): { ok: boolean; packet?: WorkerPacket; error?: string } {
		try {
			const { manifest } = opts;
			const templateRef = opts.templateRef ?? PACKET_TEMPLATE_REF;

			// Step 1a: resolve run dir (guard against traversal / empty runId)
			const dirResult = resolveRunDir(this.ledgerRoot, manifest.runId);
			if (!dirResult.ok) {
				return { ok: false, error: dirResult.error ?? "invalid runId" };
			}
			const { runDir } = dirResult;

			// Step 1b: Fix 4 — validate delegationId before using it in path/pointer construction
			if (!DLG_ID_RE.test(manifest.delegationId)) {
				return {
					ok: false,
					error: `invalid delegationId: "${manifest.delegationId}" — must match dlg-NNNN`,
				};
			}

			// Step 2: render body (includes Fix 5 single-pass + Fix 6 manifest guards)
			const renderResult = this.renderBody(manifest, opts);
			if (!renderResult.ok || !renderResult.body) {
				return { ok: false, error: renderResult.error ?? "renderBody failed" };
			}
			const { body } = renderResult;

			// Step 3: compute hash + packet path
			const promptHash = sha256Slice8(body);
			const packetPath = join(runDir, PROMPTS_DIR, `${manifest.delegationId}.md`);

			// Belt-and-suspenders path safety: verify packetPath stays inside PROMPTS_DIR
			const relToPrompts = relative(join(runDir, PROMPTS_DIR), packetPath);
			if (relToPrompts.startsWith("..") || isAbsolute(relToPrompts)) {
				return { ok: false, error: "packetPath escapes prompts directory" };
			}

			// Step 4: P9 — append packet.written event BEFORE writing the packet file
			const ev = appendLedgerEvent(this.deps, runDir, {
				type: "packet.written",
				runId: manifest.runId,
				delegationId: manifest.delegationId,
				packetPath,
				at: new Date().toISOString(),
			});
			if (!ev.ok) {
				return { ok: false, error: ev.error ?? "failed to append packet.written event" };
			}

			// Step 5: write packet body to prompts/<delegationId>.md
			this.deps.writeFileSync(packetPath, body);

			// Step 6: write prompt-trial record (Phase 2 — internally P9-correct)
			const trialResult = this.writer.writePromptTrial(manifest.runId, manifest.delegationId, {
				templateRef,
				promptHash,
			});
			if (!trialResult.ok) {
				return { ok: false, error: trialResult.error ?? "writePromptTrial failed" };
			}

			// Step 7: build pointer message
			const pointerMsg = buildPointerMsg(manifest.delegationId, packetPath, opts.repoRoot);

			const packet: WorkerPacket = {
				delegationId: manifest.delegationId,
				runId: manifest.runId,
				packetPath,
				body,
				pointerMsg,
				templateRef,
				promptHash,
			};

			return { ok: true, packet };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}
}
