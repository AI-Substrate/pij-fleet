// skills/flow-pair/lib/observe.ts
// Phase 5: Observe + diff capture.
// P2: zero @earendil-works/* | P3: ObserveDeps injected | P4: tagged-union returns | P9: event before artifacts

import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { appendLedgerEvent } from "./ledger.js";
import { resolveRunDir } from "./paths.js";

// ─── Constants (P5) ──────────────────────────────────────────────────────────

/**
 * Flow-state filenames that must never appear in a worker diff.
 * Guard matches by BASENAME (HIGH-A fix): git reports repo-relative paths like
 * "docs/plans/016-flow-pair/.the-flow-state.json" — a bare includes(f) check
 * against this list is VACUOUS for nested paths. Use basename(f) instead.
 */
const FLOW_STATE_FORBIDDEN = [".the-flow-state.json", "the-flow.json", "the-flow.md"] as const;

const DIFFS_DIR = "diffs" as const;

// ─── Types ────────────────────────────────────────────────────────────────────

export interface ObserveDeps {
	/**
	 * exitCode is included so callers can distinguish "differences found" (exit 1 for
	 * git diff --no-index) from actual errors (exit 2+). MED-2 synthetic patch generation.
	 */
	execGit(
		args: string[],
		cwd: string,
	): { ok: boolean; stdout?: string; stderr?: string; exitCode?: number };
	writeFileSync(path: string, data: string): void;
	appendFileSync(path: string, data: string): void;
	mkdirSync(path: string, opts: { recursive: boolean }): void;
	existsSync(path: string): boolean;
	readdirSync(path: string): string[];
}

export interface ObserveResult {
	diffId: string;
	runId: string;
	delegationId: string;
	changedFiles: string[];
	patchPath: string;
	statPath: string;
	manifestPath: string;
}

export interface ObserveOpts {
	repoRoot: string;
	runId: string;
	delegationId: string;
}

// ─── Production binding ───────────────────────────────────────────────────────

export function nodeObserveDeps(): ObserveDeps {
	return {
		execGit(
			args: string[],
			cwd: string,
		): { ok: boolean; stdout?: string; stderr?: string; exitCode?: number } {
			const r = spawnSync("git", args, { cwd, encoding: "utf8" });
			const exitCode = r.status ?? 128; // 128 for signal-killed processes
			// Always include stdout: callers like synthetic-patch generation need it even on exit 1.
			return {
				ok: exitCode === 0,
				stdout: r.stdout ?? "",
				stderr: r.stderr ?? r.error?.message,
				exitCode,
			};
		},
		writeFileSync(path: string, data: string): void {
			writeFileSync(path, data, "utf8");
		},
		appendFileSync(path: string, data: string): void {
			appendFileSync(path, data, "utf8");
		},
		mkdirSync(path: string, opts: { recursive: boolean }): void {
			mkdirSync(path, opts);
		},
		existsSync(path: string): boolean {
			return existsSync(path);
		},
		readdirSync(path: string): string[] {
			return readdirSync(path, { encoding: "utf8" });
		},
	};
}

// ─── NUL-delimited porcelain parser (HIGH-1) ──────────────────────────────────

/**
 * Parse `git status --porcelain=v1 -z` NUL-delimited stdout.
 *
 * HIGH-1 fix: `git status --porcelain` (without -z) QUOTES paths that contain
 * spaces: `"docs/plans/016 flow-pair/.the-flow-state.json"` (literal double-quotes).
 * parsePorcelain would then keep the quotes, making basename return
 * `.the-flow-state.json"` (trailing quote) — NOT in FLOW_STATE_FORBIDDEN → guard bypassed.
 *
 * With `--porcelain=v1 -z`, paths are NUL-separated with NO quoting, so
 * basename("docs/plans/016 flow-pair/.the-flow-state.json") = ".the-flow-state.json" → guard fires.
 *
 * Format: each record "XY PATH\0". Renames/copies: "XY NEWPATH\0ORIGPATH\0" — skip orig.
 *
 * @returns changedFiles (all modified+untracked) and untrackedFiles (subset for MED-2 patches)
 */
function parsePorcelainZ(stdout: string): { changedFiles: string[]; untrackedFiles: string[] } {
	const records = stdout.split("\0").filter((r) => r.length >= 3);
	const changedFiles: string[] = [];
	const untrackedFiles: string[] = [];
	let i = 0;
	while (i < records.length) {
		const record = records[i];
		if (record === undefined) break;
		const statusX = record[0] ?? " ";
		const statusY = record[1] ?? " ";
		const path = record.substring(3);
		if (path.length > 0) {
			changedFiles.push(path);
			if (statusX === "?" && statusY === "?") {
				untrackedFiles.push(path);
			}
		}
		// Rename/copy: the next NUL-delimited record is the ORIG path — skip it
		if (statusX === "R" || statusX === "C" || statusY === "R" || statusY === "C") {
			i += 2;
		} else {
			i++;
		}
	}
	return { changedFiles, untrackedFiles };
}

// ─── Observe class ────────────────────────────────────────────────────────────

export class Observe {
	constructor(
		readonly ledgerRoot: string,
		private readonly deps: ObserveDeps = nodeObserveDeps(),
	) {}

	capture(opts: ObserveOpts): { ok: boolean; result?: ObserveResult; error?: string } {
		// Step 1: validate runId (resolveRunDir guard)
		const dirResult = resolveRunDir(this.ledgerRoot, opts.runId);
		if (!dirResult.ok) {
			return { ok: false, error: dirResult.error ?? "invalid runId" };
		}
		const { runDir } = dirResult;

		// Step 2: changedFiles from git status --porcelain=v1 -z (HIGH-1 + HIGH-B)
		// HIGH-1: -z disables path quoting — paths with spaces are safe.
		// HIGH-B: captures untracked files that git diff HEAD --name-only misses.
		const statusResult = this.deps.execGit(["status", "--porcelain=v1", "-z"], opts.repoRoot);
		if (!statusResult.ok) {
			return { ok: false, error: statusResult.stderr ?? "git status failed" };
		}
		const { changedFiles, untrackedFiles } = parsePorcelainZ(statusResult.stdout ?? "");

		// Step 3: git diff HEAD --stat (human-readable summary for staged changes)
		const statResult = this.deps.execGit(["diff", "HEAD", "--stat"], opts.repoRoot);
		if (!statResult.ok) {
			return { ok: false, error: statResult.stderr ?? "git diff --stat failed" };
		}
		const stat = statResult.stdout ?? "";

		// Step 4: git diff HEAD (patch body for staged/tracked changes)
		const patchResult = this.deps.execGit(["diff", "HEAD"], opts.repoRoot);
		if (!patchResult.ok) {
			return { ok: false, error: patchResult.stderr ?? "git diff failed" };
		}
		let fullPatch = patchResult.stdout ?? "";

		// MED-2: append synthetic patches for untracked files (not in git diff HEAD).
		// git diff --no-index exits 1 when diffs found — treat 0 and 1 as success.
		for (const untrackedFile of untrackedFiles) {
			const synthResult = this.deps.execGit(
				["diff", "--no-index", "--", "/dev/null", untrackedFile],
				opts.repoRoot,
			);
			const ec = synthResult.exitCode ?? 2;
			if (ec <= 1) {
				fullPatch += synthResult.stdout ?? "";
			}
		}

		// Step 5 [AC-13 guard] — fires BEFORE any write (no event for contaminated diff)
		// HIGH-A: match by basename so nested paths like
		//   "docs/plans/016-flow-pair/.the-flow-state.json" are caught.
		// HIGH-1: -z ensures paths with spaces arrive unquoted — basename is correct.
		// .flow-pair/ prefix: gitignored; won't appear in normal porcelain — belt-and-suspenders.
		const forbidden = changedFiles.find(
			(f) =>
				(FLOW_STATE_FORBIDDEN as readonly string[]).includes(basename(f)) ||
				FLOW_STATE_FORBIDDEN.some((n) => f.endsWith(`/${n}`)) ||
				f.startsWith(".flow-pair/"),
		);
		if (forbidden) {
			return { ok: false, error: `observe: forbidden path in diff: ${forbidden}` };
		}

		// Step 6: allocate diffId (monotonic from .json count in diffs/)
		const diffsDir = join(runDir, DIFFS_DIR);
		const existingCount = this.deps.existsSync(diffsDir)
			? this.deps.readdirSync(diffsDir).filter((f) => f.endsWith(".json")).length
			: 0;
		const diffId = `diff-${String(existingCount + 1).padStart(4, "0")}`;

		// Step 7 [P9]: append files.changed event BEFORE any writeFileSync
		const at = new Date().toISOString();
		const ev = appendLedgerEvent(this.deps, runDir, {
			type: "files.changed",
			runId: opts.runId,
			delegationId: opts.delegationId,
			diffId,
			changedFiles,
			at,
		});
		if (!ev.ok) {
			return { ok: false, error: ev.error ?? "failed to append files.changed event" };
		}

		// Steps 8-11: mkdirSync + artifact writes
		// LOW-3: wrapped in try/catch for P4 tagged-union contract — no uncaught throws.
		// P9 preserved: files.changed event is already in events.jsonl as a recovery marker.
		const patchPath = join(diffsDir, `${diffId}.patch`);
		const statPath = join(diffsDir, `${diffId}.stat.txt`);
		const manifestPath = join(diffsDir, `${diffId}.changed-files.json`);
		try {
			this.deps.mkdirSync(diffsDir, { recursive: true });
			this.deps.writeFileSync(
				manifestPath,
				JSON.stringify(
					{
						diffId,
						runId: opts.runId,
						delegationId: opts.delegationId,
						changedFiles,
						at,
					},
					null,
					2,
				),
			);
			this.deps.writeFileSync(patchPath, fullPatch);
			this.deps.writeFileSync(statPath, stat);
		} catch (err) {
			return { ok: false, error: err instanceof Error ? err.message : String(err) };
		}

		return {
			ok: true,
			result: {
				diffId,
				runId: opts.runId,
				delegationId: opts.delegationId,
				changedFiles,
				patchPath,
				statPath,
				manifestPath,
			},
		};
	}
}
