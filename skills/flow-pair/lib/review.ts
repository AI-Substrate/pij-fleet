// skills/flow-pair/lib/review.ts
// Phase 6: Review verdict helper + fix packet generation.
// P2: zero @earendil-works/* | P3: ReviewDeps injected | P4: tagged-union returns
// P5: constants single-sourced | P9: appendLedgerEvent + {ok} check BEFORE every writeFileSync

import {
	appendFileSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	writeFileSync,
} from "node:fs";
import { isAbsolute, join, relative } from "node:path";
import { appendLedgerEvent, FIX_PACKETS_DIR, type ReviewFinding } from "./ledger.js";
import { FLOW_PAIR_SKILL_ROOT, resolveRunDir } from "./paths.js";

// ─── Constants (P5) ──────────────────────────────────────────────────────────

export const VERDICT = {
	APPROVE: "APPROVE",
	APPROVE_WITH_NOTES: "APPROVE_WITH_NOTES",
	FIX_REQUIRED: "FIX_REQUIRED",
} as const;
export type Verdict = (typeof VERDICT)[keyof typeof VERDICT];

export const FINDING_KIND = {
	ARTIFACT_CONTRACT: "artifact_contract",
	TEST_QUALITY: "test_quality",
	SCOPE: "scope",
	REGRESSION: "regression",
	OTHER: "other",
} as const;

const REVIEWS_DIR = "reviews" as const;
const REVIEW_ID_PREFIX = "rev" as const;
const FIX_PACKET_ID_PREFIX = "fix" as const;
/** Artifact files required in phaseDir; absence → artifact_contract finding → FIX_REQUIRED (AC-05). */
const REQUIRED_ARTIFACTS = ["execution.log.md"] as const;

// ─── Public types ─────────────────────────────────────────────────────────────

export interface ReviewDeps {
	existsSync(path: string): boolean;
	readFileSync(path: string, enc: "utf8"): string;
	readdirSync(path: string): string[];
	writeFileSync(path: string, data: string): void;
	appendFileSync(path: string, data: string): void;
	mkdirSync(path: string, opts: { recursive: boolean }): void;
}

export interface EvaluateOpts {
	runId: string;
	delegationId: string;
	/** Absolute path to the phase task directory; execution.log.md must be present here (AC-05). */
	phaseDir: string;
	/** Absolute repo root — makes finding `file` paths repo-relative so they work as AC-06 allowedFiles. */
	repoRoot: string;
}

export interface FixPacketOpts {
	runId: string;
	delegationId: string;
	reviewId: string;
	findings: ReviewFinding[];
	/** Absolute path to references/templates/ dir — worker-fix.md is read from here. */
	templateDir: string;
	/** Absolute repo root — for relative-path display in pointerMsg. */
	repoRoot: string;
	/**
	 * Absolute root of the installed flow-pair skill, injected as {{SKILL_ROOT}}
	 * so fix-packet citations resolve in ANY consuming repo (DL-003).
	 * Defaults to the root this module is installed under.
	 */
	skillRoot?: string;
}

export interface FixPacket {
	fixPacketId: string;
	runId: string;
	delegationId: string;
	reviewId: string;
	fixPacketPath: string;
	pointerMsg: string;
	/** AC-06: exactly the repo-relative paths extracted from findings.file (deduplicated). */
	allowedFiles: string[];
}

// ─── Production binding ───────────────────────────────────────────────────────

export function nodeReviewDeps(): ReviewDeps {
	return {
		existsSync: (path) => existsSync(path),
		readFileSync: (path, enc) => readFileSync(path, enc),
		readdirSync: (path) => readdirSync(path, { encoding: "utf8" }),
		writeFileSync: (path, data) => writeFileSync(path, data, "utf8"),
		appendFileSync: (path, data) => appendFileSync(path, data, "utf8"),
		mkdirSync: (path, opts) => mkdirSync(path, opts),
	};
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/**
 * Collect repo-relative file paths from findings where `file` is set.
 * Deduplicated. This is the SOLE source of fix-packet allowedFiles (AC-06 invariant).
 */
function extractAllowedFiles(
	findings: ReviewFinding[],
): { ok: true; allowedFiles: string[] } | { ok: false; error: string } {
	const allowedFiles: string[] = [];
	const seen = new Set<string>();

	for (const finding of findings) {
		if (finding.file == null) continue;
		if (typeof finding.file !== "string") {
			return { ok: false, error: "finding.file must be a string when present" };
		}
		const file = finding.file;
		const segments = file.split(/[\\/]+/);
		if (
			file.trim().length === 0 ||
			file === "." ||
			file.startsWith("./") ||
			isAbsolute(file) ||
			segments.includes("..")
		) {
			return { ok: false, error: `invalid finding.file path: ${file}` };
		}
		if (!seen.has(file)) {
			seen.add(file);
			allowedFiles.push(file);
		}
	}

	return { ok: true, allowedFiles };
}

/** Determine verdict from severity of findings. */
function determineVerdict(findings: ReviewFinding[]): Verdict {
	if (findings.some((f) => f.severity === "critical" || f.severity === "high")) {
		return VERDICT.FIX_REQUIRED;
	}
	if (findings.some((f) => f.severity === "medium")) {
		return VERDICT.APPROVE_WITH_NOTES;
	}
	return VERDICT.APPROVE;
}

// ─── Review class ─────────────────────────────────────────────────────────────

/**
 * Deterministic artifact-contract reviewer.
 *
 * P3: inject `ReviewDeps` via constructor; use `nodeReviewDeps()` for production.
 * P9: every writer appends the typed event (+ checks ok) BEFORE writing the state file.
 */
export class Review {
	constructor(
		readonly ledgerRoot: string,
		private readonly deps: ReviewDeps = nodeReviewDeps(),
	) {}

	/**
	 * Run deterministic artifact-contract checks and write a review record.
	 * AC-05: missing execution.log.md → verdict FIX_REQUIRED + artifact_contract finding.
	 */
	evaluate(opts: EvaluateOpts): {
		ok: boolean;
		verdict?: Verdict;
		findings?: ReviewFinding[];
		reviewId?: string;
		error?: string;
	} {
		// Step 1: validate runId
		const dirResult = resolveRunDir(this.ledgerRoot, opts.runId);
		if (!dirResult.ok) {
			return { ok: false, error: dirResult.error ?? "invalid runId" };
		}
		const { runDir } = dirResult;

		// Step 2: scaffold reviews/ dir (P4: wrap mkdirSync)
		const reviewsDir = join(runDir, REVIEWS_DIR);
		try {
			this.deps.mkdirSync(reviewsDir, { recursive: true });
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}

		// Step 3: allocate review ID — count .json files in reviews/ (OQ-01: single-writer)
		let reviewCount = 0;
		try {
			reviewCount = this.deps.readdirSync(reviewsDir).filter((f) => f.endsWith(".json")).length;
		} catch {
			reviewCount = 0;
		}
		const reviewId = `${REVIEW_ID_PREFIX}-${String(reviewCount + 1).padStart(4, "0")}`;

		// Step 4: deterministic artifact-contract checks (AC-05)
		const findings: ReviewFinding[] = [];
		for (const artifact of REQUIRED_ARTIFACTS) {
			const logExists = this.deps.existsSync(join(opts.phaseDir, artifact));
			if (!logExists) {
				findings.push({
					dimension: FINDING_KIND.ARTIFACT_CONTRACT,
					severity: "critical",
					message: `Missing required artifact: ${artifact}`,
					// repo-relative so it can be used directly in fix-packet allowedFiles (AC-06)
					file: relative(opts.repoRoot, join(opts.phaseDir, artifact)),
				});
			}
		}

		// Step 5: determine verdict — VERDICT LAW: a verdict is never minted from
		// zero findings. The deterministic sweep above only proves artifacts
		// exist; "nothing fed" must never read as "reviewed clean" (dogfood:
		// a real reviewer's FIX_REQUIRED was shadowed by a default APPROVE).
		// FIX_REQUIRED from real deterministic defects stands; APPROVE does not.
		if (findings.length === 0) {
			return {
				ok: false,
				error:
					"no findings to review — artifact-contract checks passed, but a verdict cannot be minted from zero reviewer input. Record the reviewer's real verdict via the ledger writer instead.",
			};
		}
		const verdict = determineVerdict(findings);
		const at = new Date().toISOString();

		// Step 6 [P9]: append review.recorded event — MUST succeed before writing review record
		const ev = appendLedgerEvent(this.deps, runDir, {
			type: "review.recorded",
			runId: opts.runId,
			delegationId: opts.delegationId,
			reviewId,
			verdict,
			at,
		});
		if (!ev.ok) {
			return { ok: false, error: ev.error ?? "failed to append review.recorded event" };
		}

		// Step 7 [P4 try/catch]: write review record — after P9 event
		// P9 preserved: review.recorded event is already in events.jsonl as a recovery marker
		const reviewPath = join(reviewsDir, `${reviewId}.json`);
		try {
			this.deps.writeFileSync(
				reviewPath,
				JSON.stringify(
					{
						reviewId,
						runId: opts.runId,
						delegationId: opts.delegationId,
						verdict,
						findings,
						createdAt: at,
					},
					null,
					2,
				),
			);
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}

		return { ok: true, verdict, findings, reviewId };
	}

	/**
	 * Generate a fix packet scoped to exactly the files named in findings (AC-06).
	 * Writes both fix-NNNN.json (metadata) and fix-NNNN.md (rendered template).
	 * MED-1: BOTH writes are inside the P4 try/catch, AFTER the fix_packet.written event.
	 */
	generateFixPacket(opts: FixPacketOpts): {
		ok: boolean;
		packet?: FixPacket;
		error?: string;
	} {
		// Step 1: validate runId
		const dirResult = resolveRunDir(this.ledgerRoot, opts.runId);
		if (!dirResult.ok) {
			return { ok: false, error: dirResult.error ?? "invalid runId" };
		}
		const { runDir } = dirResult;

		// Step 2: scaffold fix-packets/ dir (P4: wrap mkdirSync)
		const fixPacketsDir = join(runDir, FIX_PACKETS_DIR);
		try {
			this.deps.mkdirSync(fixPacketsDir, { recursive: true });
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}

		// Step 3: allocate fix packet ID — count .json files in fix-packets/
		let fixCount = 0;
		try {
			fixCount = this.deps.readdirSync(fixPacketsDir).filter((f) => f.endsWith(".json")).length;
		} catch {
			fixCount = 0;
		}
		const fixPacketId = `${FIX_PACKET_ID_PREFIX}-${String(fixCount + 1).padStart(4, "0")}`;

		// Step 4: extract allowed files from findings (AC-06 — sole source)
		const allowedFilesResult = extractAllowedFiles(opts.findings);
		if (!allowedFilesResult.ok) {
			return { ok: false, error: allowedFilesResult.error };
		}
		const { allowedFiles } = allowedFilesResult;

		// Step 5: read worker-fix.md template (P3: deps.readFileSync)
		let template: string;
		try {
			template = this.deps.readFileSync(join(opts.templateDir, "worker-fix.md"), "utf8");
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}

		// Step 6: render template with single-pass {{PLACEHOLDER}} substitution (Phase 4 pattern)
		const findingsSummary = opts.findings
			.map(
				(f) =>
					`- [${f.severity.toUpperCase()}] ${f.dimension}: ${f.message}${f.file ? ` (${f.file})` : ""}`,
			)
			.join("\n");
		const subs: Record<string, string> = {
			FIX_PACKET_ID: fixPacketId,
			DELEGATION_ID: opts.delegationId,
			REVIEW_ID: opts.reviewId,
			RUN_ID: opts.runId,
			// DL-003: absolute install root of the flow-pair skill for citation resolution.
			SKILL_ROOT: opts.skillRoot ?? FLOW_PAIR_SKILL_ROOT,
			ALLOWED_FILES_LIST:
				allowedFiles.length > 0 ? allowedFiles.map((f) => `- ${f}`).join("\n") : "(none)",
			FINDINGS_SUMMARY: findingsSummary || "(no findings)",
		};
		const rendered = template.replace(
			/\{\{([A-Z_]+)\}\}/g,
			(match, key: string) => subs[key] ?? match,
		);

		// Artifact paths
		const fixPacketMdPath = join(fixPacketsDir, `${fixPacketId}.md`);
		const fixPacketJsonPath = join(fixPacketsDir, `${fixPacketId}.json`);
		const pointerMsg = `[flow-pair ${fixPacketId}] Fix packet at: ${relative(opts.repoRoot, fixPacketMdPath)}`;

		const packet: FixPacket = {
			fixPacketId,
			runId: opts.runId,
			delegationId: opts.delegationId,
			reviewId: opts.reviewId,
			fixPacketPath: fixPacketMdPath,
			pointerMsg,
			allowedFiles,
		};

		// Step 7 [P9]: append fix_packet.written event — MUST succeed before any artifact write (MED-1)
		const at = new Date().toISOString();
		const ev = appendLedgerEvent(this.deps, runDir, {
			type: "fix_packet.written",
			runId: opts.runId,
			delegationId: opts.delegationId,
			reviewId: opts.reviewId,
			fixPacketId,
			fixPacketPath: fixPacketMdPath,
			allowedFiles,
			at,
		});
		if (!ev.ok) {
			return { ok: false, error: ev.error ?? "failed to append fix_packet.written event" };
		}

		// Step 8 [P4 try/catch + MED-1]: BOTH writes after the event, inside try/catch
		// P9 preserved: fix_packet.written event is already in events.jsonl as a recovery marker
		try {
			this.deps.writeFileSync(fixPacketMdPath, rendered);
			this.deps.writeFileSync(fixPacketJsonPath, JSON.stringify(packet, null, 2));
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}

		return { ok: true, packet };
	}
}
