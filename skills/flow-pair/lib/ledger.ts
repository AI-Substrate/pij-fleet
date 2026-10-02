// skills/flow-pair/lib/ledger.ts
// Phase 2: Central ledger writer — append-only, event-sourced run registry.
// P2: zero @earendil-works/* | P3: LedgerDeps injected via constructor | P7: .js ESM imports
// Single-writer v1: ID allocation via readdirSync count (OQ-01: not concurrent-safe).

import {
	appendFileSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { resolveRunDir } from "./paths.js";

// ─── Record types ─────────────────────────────────────────────────────────────

export interface RunRecord {
	runId: string;
	repoId: string;
	runDir: string;
	createdAt: string;
	status: "open" | "closed";
	closedAt?: string;
}

export interface DelegationRecord {
	delegationId: string;
	runId: string;
	taskRef: string;
	packetPath: string;
	createdAt: string;
	status: "pending" | "accepted" | "fix_required";
}

export interface PromptTrialRecord {
	trialId: string;
	runId: string;
	delegationId: string;
	templateRef: string;
	promptHash: string;
	createdAt: string;
}

export interface ReviewFinding {
	dimension: string;
	severity: "critical" | "high" | "medium" | "low" | "info";
	message: string;
	/** Repo-relative path; present when finding relates to a specific file. AC-06 anchor. */
	file?: string;
}

export interface ReviewRecord {
	reviewId: string;
	runId: string;
	delegationId: string;
	verdict: "APPROVE" | "APPROVE_WITH_NOTES" | "FIX_REQUIRED";
	findings: ReviewFinding[];
	createdAt: string;
}

export interface LearningRecord {
	learningId: string;
	runId: string;
	delegationId: string;
	cluster: string;
	candidatePath: string;
	createdAt: string;
}

export type LedgerEvent =
	| { type: "run.started"; runId: string; repoId: string; at: string }
	| { type: "run.closed"; runId: string; at: string }
	| { type: "delegation.created"; runId: string; delegationId: string; at: string }
	| {
			type: "prompt_trial.created";
			runId: string;
			delegationId: string;
			trialId: string;
			at: string;
	  }
	| { type: "review.created"; runId: string; delegationId: string; reviewId: string; at: string }
	| {
			type: "learning.created";
			runId: string;
			delegationId: string;
			learningId: string;
			at: string;
	  }
	| {
			type: "context_pack.created";
			runId: string;
			delegationId: string;
			packId: string;
			at: string;
	  }
	| { type: "packet.written"; runId: string; delegationId: string; packetPath: string; at: string }
	| {
			type: "files.changed";
			runId: string;
			delegationId: string;
			diffId: string;
			changedFiles: string[];
			at: string;
	  }
	| {
			type: "review.recorded";
			runId: string;
			delegationId: string;
			reviewId: string;
			verdict: string;
			at: string;
	  }
	| {
			type: "fix_packet.written";
			runId: string;
			delegationId: string;
			reviewId: string;
			fixPacketId: string;
			fixPacketPath: string;
			allowedFiles: string[];
			at: string;
	  };

// ─── Constants (P5) ──────────────────────────────────────────────────────────

/** Sub-directory under the run directory holding rendered worker packets. Exported for Phase 4+. */
export const PROMPTS_DIR = "prompts" as const;
/** Sub-directory under the run directory holding fix packets. Exported for Phase 6. */
export const FIX_PACKETS_DIR = "fix-packets" as const;

const EVENTS_FILE = "events.jsonl" as const;
const RUN_JSON_FILE = "run.json" as const;
const DELEGATIONS_DIR = "delegations" as const;
const PROMPT_TRIALS_DIR = "prompt-trials" as const;
const REVIEWS_DIR = "reviews" as const;
const LEARNINGS_DIR = "learnings" as const;

/** Subdirectories scaffolded by createRun. Phases 4-5 write into prompts/worker-reports/diffs/. */
const RUN_SUBDIRS = [
	"delegations",
	"prompt-trials",
	"reviews",
	"learnings",
	"prompts",
	"worker-reports",
	"diffs",
	"fix-packets",
] as const;

const ID_PREFIXES = {
	delegation: "dlg",
	promptTrial: "trial",
	review: "rev",
	learning: "learn",
} as const;

// ─── P3 injectable fs deps ────────────────────────────────────────────────────

export interface LedgerDeps {
	mkdirSync(path: string, opts?: { recursive?: boolean }): void;
	writeFileSync(path: string, data: string): void;
	appendFileSync(path: string, data: string): void;
	readFileSync(path: string, enc: "utf8"): string;
	existsSync(path: string): boolean;
	readdirSync(path: string): string[];
}

/** Production binding — wraps node:fs. */
export function nodeLedgerDeps(): LedgerDeps {
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

// ─── Internal helpers ─────────────────────────────────────────────────────────

/**
 * Standalone helper: append a single event line to events.jsonl.
 * Exported so Phase 3+ can reuse the tested primitive without importing LedgerWriter.
 * P9 primitive: callers must invoke this BEFORE writing any state file.
 */
export function appendLedgerEvent(
	deps: Pick<LedgerDeps, "appendFileSync">,
	runDir: string,
	event: LedgerEvent,
): { ok: boolean; error?: string } {
	try {
		deps.appendFileSync(join(runDir, EVENTS_FILE), `${JSON.stringify(event)}\n`);
		return { ok: true };
	} catch (err) {
		return { ok: false, error: err instanceof Error ? err.message : String(err) };
	}
}

/** Allocate next monotonic ID for a record type in the given sub-directory. */
function nextId(dir: string, prefix: string, deps: LedgerDeps): string {
	const count = deps.readdirSync(dir).filter((f) => f.endsWith(".json")).length;
	return `${prefix}-${String(count + 1).padStart(4, "0")}`;
}

// ─── LedgerWriter ─────────────────────────────────────────────────────────────

/**
 * Append-only ledger writer.
 *
 * P3: inject `LedgerDeps` via constructor; use `nodeLedgerDeps()` for production.
 * P9: every writer appends the typed event BEFORE writing the state file.
 * OQ-01: single-writer assumption — ID allocation via readdirSync count is not concurrent-safe.
 */
export class LedgerWriter {
	constructor(
		readonly ledgerRoot: string,
		private readonly deps: LedgerDeps = nodeLedgerDeps(),
	) {}

	// ─── Run lifecycle ──────────────────────────────────────────────────────────

	/**
	 * Create a new run: scaffold 7 subdirs, append run.started (P9 first), write run.json.
	 * runId format: <YYYY-MM-DDTHH-MM-SSZ>-<repoId[0:20]>
	 */
	createRun(repoId: string): { ok: boolean; run?: RunRecord; error?: string } {
		try {
			const ts = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19).concat("Z");
			const runId = `${ts}-${repoId.slice(0, 20)}`;

			const dirResult = resolveRunDir(this.ledgerRoot, runId);
			if (!dirResult.ok) {
				return { ok: false, error: dirResult.error ?? "failed to resolve run dir" };
			}
			const { runDir } = dirResult;

			// F3: collision guard — abort before any mutation if runDir already exists
			if (this.deps.existsSync(runDir)) {
				return { ok: false, error: `run directory already exists: ${runId}` };
			}

			// Scaffold run dir + 7 subdirs
			this.deps.mkdirSync(runDir, { recursive: true });
			for (const subdir of RUN_SUBDIRS) {
				this.deps.mkdirSync(join(runDir, subdir), { recursive: true });
			}

			const createdAt = new Date().toISOString();

			// P9: event BEFORE state file; F1: check {ok} before writing state
			const evCreate = this.appendEvent(runDir, {
				type: "run.started",
				runId,
				repoId,
				at: createdAt,
			});
			if (!evCreate.ok)
				return { ok: false, error: evCreate.error ?? "failed to append run.started event" };

			const run: RunRecord = { runId, repoId, runDir, createdAt, status: "open" };
			this.deps.writeFileSync(join(runDir, RUN_JSON_FILE), JSON.stringify(run, null, 2));

			return { ok: true, run };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}

	/**
	 * Close a run: append run.closed (P9 first), update run.json with status:"closed" + closedAt.
	 */
	closeRun(runId: string): { ok: boolean; error?: string } {
		try {
			const dirResult = resolveRunDir(this.ledgerRoot, runId);
			if (!dirResult.ok) return { ok: false, error: dirResult.error };
			const { runDir } = dirResult;

			// F2: read + validate BEFORE appending (read is not a mutation; P9 still holds
			// because the event still precedes the write). Prevents a false run.closed event
			// when run.json is missing or malformed.
			const existing = JSON.parse(
				this.deps.readFileSync(join(runDir, RUN_JSON_FILE), "utf8"),
			) as RunRecord;

			const closedAt = new Date().toISOString();

			// P9: event BEFORE state write; F1: check {ok}
			const evClose = this.appendEvent(runDir, { type: "run.closed", runId, at: closedAt });
			if (!evClose.ok)
				return { ok: false, error: evClose.error ?? "failed to append run.closed event" };
			const updated: RunRecord = { ...existing, status: "closed", closedAt };
			this.deps.writeFileSync(join(runDir, RUN_JSON_FILE), JSON.stringify(updated, null, 2));

			return { ok: true };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}

	/**
	 * Append a single event line to events.jsonl (the only write path to the event log).
	 * Delegates to the exported `appendLedgerEvent` standalone helper.
	 */
	appendEvent(runDir: string, event: LedgerEvent): { ok: boolean; error?: string } {
		return appendLedgerEvent(this.deps, runDir, event);
	}

	// ─── Record writers ─────────────────────────────────────────────────────────

	/**
	 * Write a delegation record.
	 * P9: appends delegation.created event before writing delegations/<id>.json.
	 */
	writeDelegation(
		runId: string,
		opts: { taskRef: string; packetPath: string },
	): { ok: boolean; delegation?: DelegationRecord; error?: string } {
		try {
			const dirResult = resolveRunDir(this.ledgerRoot, runId);
			if (!dirResult.ok) return { ok: false, error: dirResult.error };
			const { runDir } = dirResult;

			const delegationsDir = join(runDir, DELEGATIONS_DIR);
			const delegationId = nextId(delegationsDir, ID_PREFIXES.delegation, this.deps);
			const createdAt = new Date().toISOString();

			// P9: event first; F1: check {ok} before writing record
			const evDlg = this.appendEvent(runDir, {
				type: "delegation.created",
				runId,
				delegationId,
				at: createdAt,
			});
			if (!evDlg.ok)
				return { ok: false, error: evDlg.error ?? "failed to append delegation.created" };

			const delegation: DelegationRecord = {
				delegationId,
				runId,
				taskRef: opts.taskRef,
				packetPath: opts.packetPath,
				createdAt,
				status: "pending",
			};
			this.deps.writeFileSync(
				join(delegationsDir, `${delegationId}.json`),
				JSON.stringify(delegation, null, 2),
			);

			return { ok: true, delegation };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}

	/**
	 * Write a prompt-trial record.
	 * P9: appends prompt_trial.created event before writing prompt-trials/<id>.json.
	 */
	writePromptTrial(
		runId: string,
		delegationId: string,
		opts: { templateRef: string; promptHash: string },
	): { ok: boolean; trial?: PromptTrialRecord; error?: string } {
		try {
			const dirResult = resolveRunDir(this.ledgerRoot, runId);
			if (!dirResult.ok) return { ok: false, error: dirResult.error };
			const { runDir } = dirResult;

			const trialsDir = join(runDir, PROMPT_TRIALS_DIR);
			const trialId = nextId(trialsDir, ID_PREFIXES.promptTrial, this.deps);
			const createdAt = new Date().toISOString();

			// P9: event first; F1: check {ok} before writing record
			const evTrial = this.appendEvent(runDir, {
				type: "prompt_trial.created",
				runId,
				delegationId,
				trialId,
				at: createdAt,
			});
			if (!evTrial.ok)
				return { ok: false, error: evTrial.error ?? "failed to append prompt_trial.created" };

			const trial: PromptTrialRecord = {
				trialId,
				runId,
				delegationId,
				templateRef: opts.templateRef,
				promptHash: opts.promptHash,
				createdAt,
			};
			this.deps.writeFileSync(join(trialsDir, `${trialId}.json`), JSON.stringify(trial, null, 2));

			return { ok: true, trial };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}

	/**
	 * Write a review record.
	 * P9: appends review.created event before writing reviews/<id>.json.
	 */
	writeReview(
		runId: string,
		delegationId: string,
		opts: { verdict: ReviewRecord["verdict"]; findings: ReviewFinding[] },
	): { ok: boolean; review?: ReviewRecord; error?: string } {
		try {
			const dirResult = resolveRunDir(this.ledgerRoot, runId);
			if (!dirResult.ok) return { ok: false, error: dirResult.error };
			const { runDir } = dirResult;

			const reviewsDir = join(runDir, REVIEWS_DIR);
			const reviewId = nextId(reviewsDir, ID_PREFIXES.review, this.deps);
			const createdAt = new Date().toISOString();

			// P9: event first; F1: check {ok} before writing record
			const evRev = this.appendEvent(runDir, {
				type: "review.created",
				runId,
				delegationId,
				reviewId,
				at: createdAt,
			});
			if (!evRev.ok) return { ok: false, error: evRev.error ?? "failed to append review.created" };

			const review: ReviewRecord = {
				reviewId,
				runId,
				delegationId,
				verdict: opts.verdict,
				findings: opts.findings,
				createdAt,
			};
			this.deps.writeFileSync(
				join(reviewsDir, `${reviewId}.json`),
				JSON.stringify(review, null, 2),
			);

			return { ok: true, review };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}

	/**
	 * Write a learning record.
	 * P9: appends learning.created event before writing learnings/<id>.json.
	 */
	writeLearning(
		runId: string,
		delegationId: string,
		opts: { cluster: string; candidatePath: string },
	): { ok: boolean; learning?: LearningRecord; error?: string } {
		try {
			const dirResult = resolveRunDir(this.ledgerRoot, runId);
			if (!dirResult.ok) return { ok: false, error: dirResult.error };
			const { runDir } = dirResult;

			const learningsDir = join(runDir, LEARNINGS_DIR);
			const learningId = nextId(learningsDir, ID_PREFIXES.learning, this.deps);
			const createdAt = new Date().toISOString();

			// P9: event first; F1: check {ok} before writing record
			const evLearn = this.appendEvent(runDir, {
				type: "learning.created",
				runId,
				delegationId,
				learningId,
				at: createdAt,
			});
			if (!evLearn.ok)
				return { ok: false, error: evLearn.error ?? "failed to append learning.created" };

			const learning: LearningRecord = {
				learningId,
				runId,
				delegationId,
				cluster: opts.cluster,
				candidatePath: opts.candidatePath,
				createdAt,
			};
			this.deps.writeFileSync(
				join(learningsDir, `${learningId}.json`),
				JSON.stringify(learning, null, 2),
			);

			return { ok: true, learning };
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}
	}
}
