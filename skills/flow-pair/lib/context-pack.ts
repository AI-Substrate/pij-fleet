// skills/flow-pair/lib/context-pack.ts
// Phase 3: Context-pack compiler — full implementation (T006).
// P2: node:fs/path/crypto only | P3: ContextPackDeps injected | P7: .js ESM
// P9: appendLedgerEvent (from lib/ledger.ts) called BEFORE writeFileSync

import { createHash } from "node:crypto";
import {
	appendFileSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { appendLedgerEvent } from "./ledger.js";
import { FLOW_PAIR_SKILL_ROOT, resolveRunDir } from "./paths.js";

// ─── P3 injectable fs deps ────────────────────────────────────────────────────

export interface ContextPackDeps {
	readFileSync(path: string, enc: "utf8"): string;
	existsSync(path: string): boolean;
	readdirSync(path: string): string[];
	writeFileSync(path: string, data: string): void;
	appendFileSync(path: string, data: string): void;
	mkdirSync(path: string, opts?: { recursive?: boolean }): void;
}

/** Production binding — wraps node:fs. */
export function nodeContextPackDeps(): ContextPackDeps {
	return {
		readFileSync: (path, enc) => readFileSync(path, enc),
		existsSync: (path) => existsSync(path),
		readdirSync: (path) => readdirSync(path, { encoding: "utf8" }),
		writeFileSync: (path, data) => writeFileSync(path, data, "utf8"),
		appendFileSync: (path, data) => appendFileSync(path, data, "utf8"),
		mkdirSync: (path, opts) => mkdirSync(path, opts),
	};
}

// ─── Constants (P5) ──────────────────────────────────────────────────────────

export const CONTEXT_PACKS_DIR = "context-packs" as const;
export const PACK_ID_PREFIX = "cp" as const;

/**
 * Paths the worker must never modify.
 * Implementation note: spread when assigning to string[] to avoid `readonly` conflict:
 *   `forbiddenPaths: opts.forbiddenPaths ?? [...DEFAULT_FORBIDDEN_PATHS]`
 */
export const DEFAULT_FORBIDDEN_PATHS = [
	".the-flow-state.json",
	"the-flow.json",
	"the-flow.md",
	".flow-pair/",
] as const;

// ─── Types ────────────────────────────────────────────────────────────────────

export interface ContextPackEntry {
	path: string;
	section?: string;
	/** Extracted text content — Phase 4 renders from this; no re-read needed. */
	content: string;
	/** sha256[0:8] of `content`. */
	hash: string;
	role: "plan-phase" | "tasks" | "execution-log" | "learning";
}

export interface ContextPackExclusion {
	path: string;
	/** Controlled vocabulary: "not found" | "wrong cluster" | "other phase" */
	reason: string;
}

export interface ContextPackManifest {
	packId: string;
	runId: string;
	delegationId: string;
	phase: string;
	cluster: string;
	entries: ContextPackEntry[];
	exclusions: ContextPackExclusion[];
	allowedPaths: string[];
	forbiddenPaths: string[];
	createdAt: string;
}

export interface ClusterLearning {
	cluster: string;
	sourcePath: string;
	content: string;
}

export interface CompileOpts {
	runId: string;
	delegationId: string;
	/** Absolute path to the plan markdown file. */
	planPath: string;
	/** Section heading to extract (exact, prefix-colon, or prefix-space match). */
	phase: string;
	/** Absolute path to tasks directory (looks for tasks.md + execution.log.md inside). */
	tasksDir: string;
	cluster: string;
	allowedPaths: string[];
	forbiddenPaths?: string[];
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

function sha256slice8(content: string): string {
	return createHash("sha256").update(content).digest("hex").slice(0, 8);
}

function nextPackId(packDir: string, deps: ContextPackDeps): string {
	const count = deps.readdirSync(packDir).filter((f) => f.endsWith(".json")).length;
	return `${PACK_ID_PREFIX}-${String(count + 1).padStart(4, "0")}`;
}

// ─── ContextPackCompiler ──────────────────────────────────────────────────────

/**
 * Context-pack compiler: reads plan/task/log files, extracts only what the
 * worker needs, assembles a durable manifest (P9-written), and returns
 * same-cluster learnings (empty list when Phase 7 is not yet built).
 *
 * P3: inject `ContextPackDeps` via constructor; use `nodeContextPackDeps()` for production.
 * P9: `appendLedgerEvent` is called BEFORE `writeFileSync` in every compile path.
 */
export class ContextPackCompiler {
	constructor(
		readonly repoRoot: string,
		readonly ledgerRoot: string,
		private readonly deps: ContextPackDeps = nodeContextPackDeps(),
		/**
		 * Absolute root of the installed flow-pair skill (DL-003). Cluster learnings
		 * live under `<skillRoot>/prompt-lab/` — NOT under the consuming repo's root.
		 */
		readonly skillRoot: string = FLOW_PAIR_SKILL_ROOT,
	) {}

	/**
	 * Extract a named section from a markdown file (includes subsections).
	 *
	 * Match rule (prefix-boundary, NOT substring):
	 *   norm === target  OR  norm.startsWith(target + ":")  OR  norm.startsWith(target + " ")
	 * where norm = heading-text.trim().toLowerCase()
	 *
	 * Prevents "Phase 1" matching "Phase 10" when Phase 10 appears first.
	 *
	 * @returns {ok:false} when file not found or section not found.
	 */
	extractSection(
		filePath: string,
		sectionHeading: string,
	): { ok: boolean; content?: string; error?: string } {
		if (!sectionHeading.trim()) {
			return { ok: false, error: "sectionHeading must not be empty" };
		}

		let raw: string;
		try {
			raw = this.deps.readFileSync(filePath, "utf8");
		} catch {
			return { ok: false, error: `not found: ${filePath}` };
		}

		const lines = raw.split("\n");
		const target = sectionHeading.trim().toLowerCase();

		// Find first heading matching prefix-boundary rule
		let matchIdx = -1;
		let matchLevel = 0;
		for (let i = 0; i < lines.length; i++) {
			const line = lines[i] ?? "";
			if (!line.startsWith("#")) continue;
			const norm = line
				.replace(/^#+\s*/, "")
				.trim()
				.toLowerCase();
			if (norm === target || norm.startsWith(`${target}:`) || norm.startsWith(`${target} `)) {
				matchIdx = i;
				matchLevel = (line.match(/^#+/) ?? [""])[0]?.length ?? 1;
				break;
			}
		}

		if (matchIdx < 0) {
			return { ok: false, error: `section not found: ${sectionHeading}` };
		}

		// Collect section lines until next heading of same or higher level
		const collected: string[] = [lines[matchIdx] ?? ""];
		for (let i = matchIdx + 1; i < lines.length; i++) {
			const line = lines[i] ?? "";
			if (line.startsWith("#")) {
				const level = (line.match(/^#+/) ?? [""])[0]?.length ?? 1;
				if (level <= matchLevel) break;
			}
			collected.push(line);
		}

		return { ok: true, content: collected.join("\n").trim() };
	}

	/**
	 * Return same-cluster learnings from the SKILL's prompt-lab (DL-003: resolved
	 * from `skillRoot`, never the consuming repo root — repoRoot-joined lookups
	 * silently compiled empty in every repo other than pij).
	 *
	 * Always {ok:true}: returns [] when cluster dir or active.md is absent, with
	 * `searchedPath` naming the active.md that was looked for so callers can
	 * record the miss honestly instead of dropping it.
	 */
	clusterLearnings(cluster: string): {
		ok: boolean;
		learnings?: ClusterLearning[];
		/** The active.md path that was searched — set even when nothing was found. */
		searchedPath?: string;
		error?: string;
	} {
		const clusterPath = join(this.skillRoot, "prompt-lab", "clusters", cluster);
		const activePath = join(clusterPath, "active.md");
		if (!this.deps.existsSync(clusterPath)) {
			return { ok: true, learnings: [], searchedPath: activePath };
		}
		if (!this.deps.existsSync(activePath)) {
			return { ok: true, learnings: [], searchedPath: activePath };
		}
		const content = this.deps.readFileSync(activePath, "utf8");
		return {
			ok: true,
			learnings: [{ cluster, sourcePath: activePath, content }],
			searchedPath: activePath,
		};
	}

	/**
	 * Compile a context-pack manifest for a delegation.
	 *
	 * Algorithm (order matters for P9):
	 *  1. resolveRunDir — {ok:false} on bad runId
	 *  2. extractSection(planPath, phase) — {ok:false} on file-missing or section-absent
	 *  3. Read tasks.md → entry; or exclusion with reason:"not found"
	 *  4. Read execution.log.md → entry; or exclusion with reason:"not found"
	 *  5. clusterLearnings — [] if Phase 7 not built
	 *  6. Build entries (with content + hash), exclusions, manifest struct
	 *  7. deps.mkdirSync(packDir) — BEFORE readdirSync (packDir must exist first)
	 *  8. nextPackId(packDir) via readdirSync
	 *  9. [P9] appendLedgerEvent → if !ok: return {ok:false} (writeFileSync never called)
	 * 10. deps.writeFileSync(cp-NNNN.json)
	 */
	compile(opts: CompileOpts): { ok: boolean; manifest?: ContextPackManifest; error?: string } {
		try {
			// Step 1: validate runId
			const dirResult = resolveRunDir(this.ledgerRoot, opts.runId);
			if (!dirResult.ok) {
				return { ok: false, error: dirResult.error ?? "failed to resolve run dir" };
			}
			const { runDir } = dirResult;

			// Step 2: extract plan phase section
			const extract = this.extractSection(opts.planPath, opts.phase);
			if (!extract.ok) {
				return { ok: false, error: extract.error };
			}

			const entries: ContextPackEntry[] = [];
			const exclusions: ContextPackExclusion[] = [];

			const planContent = extract.content ?? "";
			entries.push({
				path: opts.planPath,
				section: opts.phase,
				content: planContent,
				hash: sha256slice8(planContent),
				role: "plan-phase",
			});

			// Step 3: tasks.md
			const tasksPath = join(opts.tasksDir, "tasks.md");
			if (this.deps.existsSync(tasksPath)) {
				const tasksContent = this.deps.readFileSync(tasksPath, "utf8");
				entries.push({
					path: tasksPath,
					content: tasksContent,
					hash: sha256slice8(tasksContent),
					role: "tasks",
				});
			} else {
				exclusions.push({ path: tasksPath, reason: "not found" });
			}

			// Step 4: execution.log.md
			const logPath = join(opts.tasksDir, "execution.log.md");
			if (this.deps.existsSync(logPath)) {
				const logContent = this.deps.readFileSync(logPath, "utf8");
				entries.push({
					path: logPath,
					content: logContent,
					hash: sha256slice8(logContent),
					role: "execution-log",
				});
			} else {
				exclusions.push({ path: logPath, reason: "not found" });
			}

			// Step 5: cluster learnings — resolved from skillRoot (DL-003). An empty
			// result is recorded as an exclusion (honest miss), never dropped silently.
			const learnResult = this.clusterLearnings(opts.cluster);
			if (learnResult.ok && learnResult.learnings && learnResult.learnings.length > 0) {
				for (const learning of learnResult.learnings) {
					entries.push({
						path: learning.sourcePath,
						content: learning.content,
						hash: sha256slice8(learning.content),
						role: "learning",
					});
				}
			} else if (learnResult.searchedPath) {
				exclusions.push({ path: learnResult.searchedPath, reason: "not found" });
			}

			const createdAt = new Date().toISOString();

			// Steps 7-8: mkdirSync packDir FIRST, then nextPackId (needs readdirSync)
			const packDir = join(runDir, CONTEXT_PACKS_DIR);
			this.deps.mkdirSync(packDir, { recursive: true });
			const packId = nextPackId(packDir, this.deps);

			const manifest: ContextPackManifest = {
				packId,
				runId: opts.runId,
				delegationId: opts.delegationId,
				phase: opts.phase,
				cluster: opts.cluster,
				entries,
				exclusions,
				allowedPaths: opts.allowedPaths,
				forbiddenPaths: opts.forbiddenPaths
					? [...opts.forbiddenPaths]
					: [...DEFAULT_FORBIDDEN_PATHS],
				createdAt,
			};

			// Step 9: [P9] appendLedgerEvent BEFORE writeFileSync
			const ev = appendLedgerEvent(this.deps, runDir, {
				type: "context_pack.created",
				runId: opts.runId,
				delegationId: opts.delegationId,
				packId,
				at: createdAt,
			});
			if (!ev.ok) {
				return { ok: false, error: ev.error ?? "failed to append context_pack.created event" };
			}

			// Step 10: write manifest
			this.deps.writeFileSync(join(packDir, `${packId}.json`), JSON.stringify(manifest, null, 2));

			return { ok: true, manifest };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}
}
