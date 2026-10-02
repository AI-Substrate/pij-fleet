// skills/flow-pair/lib/paths.ts
// P2: zero @earendil-works/* imports | P7: .js ESM imports

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// ─── Constants (P5) ──────────────────────────────────────────────────────────

/** Default ledger root directory (gitignored at repo root). */
export const LEDGER_ROOT = ".flow-pair" as const;

/**
 * Absolute root of the INSTALLED flow-pair skill (the parent of `lib/`,
 * `references/`, and `prompt-lab/`), resolved from this module's location.
 *
 * DL-003 (packet path drift): the skill runs against arbitrary consuming repos,
 * so skill-owned assets (templates, references, prompt-lab clusters) must be
 * resolved from THIS root — never joined onto the consuming repo's root.
 */
export const FLOW_PAIR_SKILL_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Sub-directory under ledger root that holds per-run directories. */
export const RUNS_DIR = "runs" as const;

// ─── Public API ───────────────────────────────────────────────────────────────

/**
 * Resolve the run directory path.
 *
 * Output layout: `<ledgerRoot>/runs/<runId>/`
 *
 * @returns Tagged-union `{ ok, runDir }` (P4).
 *          Returns `ok: false` for: empty runId, whitespace-only, absolute
 *          path, or a runId containing `/` or `..` (path-traversal guard).
 */
export function resolveRunDir(
	ledgerRoot: string,
	runId: string,
): { ok: boolean; runDir: string; error?: string } {
	if (runId.trim().length === 0) {
		return { ok: false, runDir: "", error: "runId must not be empty" };
	}
	if (runId.startsWith("/")) {
		return { ok: false, runDir: "", error: "runId must not be an absolute path" };
	}
	if (runId.includes("/") || runId.includes("..")) {
		return {
			ok: false,
			runDir: "",
			error: "runId must not contain path separators or '..'",
		};
	}
	return { ok: true, runDir: join(ledgerRoot, RUNS_DIR, runId) };
}
