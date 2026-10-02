// skills/flow-pair/lib/learning.ts
// Phase 7: Prompt-learning notes + cluster lifecycle.
// P2: zero @earendil-works/* imports | P3: injected side effects | P7: .js ESM imports

import {
	appendFileSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { join, resolve, sep } from "node:path";
import { type LearningRecord, LedgerWriter } from "./ledger.js";
import { resolveRunDir } from "./paths.js";

// ─── Constants (P5) ──────────────────────────────────────────────────────────

export const PROMPT_CLUSTERS = [
	"implement-code",
	"fix-code",
	"review-code",
	"docs-writing",
	"codebase-research",
	"validation-runner",
] as const;

export type PromptCluster = (typeof PROMPT_CLUSTERS)[number];

const CLUSTERS_DIR = "clusters" as const;
const CANDIDATES_DIR = "candidates" as const;
const ACTIVE_FILE = "active.md" as const;
const CHANGELOG_FILE = "changelog.md" as const;
const LEARNINGS_DIR = "learnings" as const;
const LEARNING_PREFIX = "learn" as const;

// ─── Public types ────────────────────────────────────────────────────────────

export interface LearningDeps {
	mkdirSync(path: string, opts?: { recursive?: boolean }): void;
	writeFileSync(path: string, data: string): void;
	appendFileSync(path: string, data: string): void;
	readFileSync(path: string, enc: "utf8"): string;
	existsSync(path: string): boolean;
	readdirSync(path: string): string[];
}

export interface RecordLearningOpts {
	runId: string;
	delegationId: string;
	cluster: PromptCluster;
	/** v1 redundancy: missType must equal cluster; kept as a forward slot for later many-to-one attribution. */
	missType: PromptCluster;
	summary: string;
	evidence: string[];
	candidateDelta: string;
	promptLabRoot: string;
}

export interface LearningCandidate {
	learningId: string;
	cluster: PromptCluster;
	candidatePath: string;
	ledgerRecordPath: string;
}

export type RecordLearningResult =
	| { ok: true; candidate: LearningCandidate }
	| { ok: false; error: string };

// ─── Production deps ─────────────────────────────────────────────────────────

export function nodeLearningDeps(): LearningDeps {
	return {
		mkdirSync(path: string, opts?: { recursive?: boolean }): void {
			mkdirSync(path, opts);
		},
		writeFileSync(path: string, data: string): void {
			writeFileSync(path, data, "utf8");
		},
		appendFileSync(path: string, data: string): void {
			appendFileSync(path, data, "utf8");
		},
		readFileSync(path: string, enc: "utf8"): string {
			return readFileSync(path, enc);
		},
		existsSync(path: string): boolean {
			return existsSync(path);
		},
		readdirSync(path: string): string[] {
			return readdirSync(path, { encoding: "utf8" });
		},
	};
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

export function isPromptCluster(value: string): value is PromptCluster {
	return (PROMPT_CLUSTERS as readonly string[]).includes(value);
}

function nextLearningId(learningsDir: string, deps: LearningDeps): string {
	const count = deps.readdirSync(learningsDir).filter((f) => f.endsWith(".json")).length;
	return `${LEARNING_PREFIX}-${String(count + 1).padStart(4, "0")}`;
}

function renderCandidateMarkdown(
	opts: RecordLearningOpts,
	learningId: string,
	createdAt: string,
): string {
	const evidenceLines =
		opts.evidence.length === 0
			? "- (none provided)"
			: opts.evidence.map((item) => `- ${item}`).join("\n");
	return `# Learning Candidate — ${learningId}

- **Cluster**: ${opts.cluster}
- **Run**: ${opts.runId}
- **Delegation**: ${opts.delegationId}
- **Miss type**: ${opts.missType}
- **Created at**: ${createdAt}

## Summary

${opts.summary}

## Evidence

${evidenceLines}

## Candidate prompt delta

${opts.candidateDelta}

## Promotion status

Pending manual review. No automatic promotion: do not edit \`active.md\` automatically. Record any promotion decision in \`changelog.md\`.
`;
}

function lifecyclePathMissing(clusterDir: string, deps: LearningDeps): string | undefined {
	const candidatesDir = join(clusterDir, CANDIDATES_DIR);
	for (const path of [
		clusterDir,
		candidatesDir,
		join(clusterDir, ACTIVE_FILE),
		join(clusterDir, CHANGELOG_FILE),
	]) {
		if (!deps.existsSync(path)) return path;
	}
	return undefined;
}

function toLearningRecordPath(runDir: string, learningId: string): string {
	return join(runDir, LEARNINGS_DIR, `${learningId}.json`);
}

function normalizeEvidence(evidence: string[]): string[] {
	return evidence.map((item) => item.trim()).filter((item) => item.length > 0);
}

// ─── Learning writer ─────────────────────────────────────────────────────────

export class Learning {
	constructor(
		readonly ledgerRoot: string,
		private readonly deps: LearningDeps = nodeLearningDeps(),
	) {}

	recordLearning(opts: RecordLearningOpts): RecordLearningResult {
		try {
			const ledgerRoot = resolve(this.ledgerRoot);
			const dirResult = resolveRunDir(ledgerRoot, opts.runId);
			if (!dirResult.ok) {
				return { ok: false, error: dirResult.error ?? "invalid run id" };
			}

			if (!isPromptCluster(opts.cluster)) {
				return { ok: false, error: `cluster must be one of: ${PROMPT_CLUSTERS.join(", ")}` };
			}
			// v1: missType is intentionally redundant; it must equal cluster until many-to-one attribution exists.
			if (opts.missType !== opts.cluster) {
				return { ok: false, error: "missType must match cluster in v1" };
			}

			const promptLabRoot = resolve(opts.promptLabRoot);
			const clusterDir = join(promptLabRoot, CLUSTERS_DIR, opts.cluster);
			const candidatesDir = join(clusterDir, "candidates");
			const missingPath = lifecyclePathMissing(clusterDir, this.deps);
			if (missingPath !== undefined) {
				return { ok: false, error: `missing prompt-lab lifecycle path: ${missingPath}` };
			}

			const learningsDir = join(dirResult.runDir, LEARNINGS_DIR);
			if (!this.deps.existsSync(learningsDir)) {
				return { ok: false, error: `missing run learnings directory: ${learningsDir}` };
			}

			const learningId = nextLearningId(learningsDir, this.deps);
			const candidatePath = resolve(candidatesDir, `${learningId}.md`);
			const resolvedCandidatesDir = resolve(candidatesDir);
			if (!candidatePath.startsWith(resolvedCandidatesDir + sep)) {
				return { ok: false, error: "candidate path escaped selected cluster candidates directory" };
			}

			const createdAt = new Date().toISOString();
			const normalizedOpts: RecordLearningOpts = {
				...opts,
				evidence: normalizeEvidence(opts.evidence),
			};
			const markdown = renderCandidateMarkdown(normalizedOpts, learningId, createdAt);

			// P9: persist the per-run learning event + record before mutating prompt-lab candidates.
			// If a crash happens after this point but before candidate write, the ledger intentionally
			// retains an auditable intent record for manual recovery.
			const writer = new LedgerWriter(ledgerRoot, this.deps);
			const ledgerResult = writer.writeLearning(opts.runId, opts.delegationId, {
				cluster: opts.cluster,
				candidatePath,
			});
			if (!ledgerResult.ok) {
				return { ok: false, error: ledgerResult.error ?? "failed to write learning record" };
			}

			this.deps.writeFileSync(candidatePath, markdown);

			const candidate: LearningCandidate = {
				learningId,
				cluster: opts.cluster,
				candidatePath,
				ledgerRecordPath: toLearningRecordPath(dirResult.runDir, learningId),
			};
			return { ok: true, candidate };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}
}

export type { LearningRecord };
