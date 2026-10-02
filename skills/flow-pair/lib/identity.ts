// skills/flow-pair/lib/identity.ts
// P2: zero @earendil-works/* imports | P3: inject GitDeps | P7: .js ESM imports

import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { basename } from "node:path";

// ─── Constants (P5) ──────────────────────────────────────────────────────────

/** Regex patterns to parse git remote URLs into host-owner-repo segments. */
const REMOTE_PATTERNS: ReadonlyArray<RegExp> = [
	// HTTPS:  https://github.com/owner/repo.git
	/^https?:\/\/([^/:]+)\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/,
	// SSH:    git@github.com:owner/repo.git
	/^git@([^:]+):([^/]+)\/([^/]+?)(?:\.git)?\/?$/,
];

/** Number of hex characters to use from the sha256 path-hash fallback. */
const HASH_LENGTH = 8 as const;

// ─── GitDeps interface (P3) ───────────────────────────────────────────────────

export interface GitDeps {
	getRemoteOriginUrl(repoPath: string): string | null;
}

/** Production GitDeps implementation — calls git CLI. */
export function nodeGitDeps(): GitDeps {
	return {
		getRemoteOriginUrl(repoPath: string): string | null {
			try {
				const url = execSync("git remote get-url origin", {
					cwd: repoPath,
					encoding: "utf8",
					stdio: ["pipe", "pipe", "pipe"],
				}).trim();
				return url.length > 0 ? url : null;
			} catch {
				return null;
			}
		},
	};
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

function parseRemoteUrl(url: string): string | null {
	for (const pattern of REMOTE_PATTERNS) {
		const m = pattern.exec(url);
		if (m !== null) {
			const [, host, owner, repo] = m;
			if (host !== undefined && owner !== undefined && repo !== undefined) {
				return `${host}-${owner}-${repo}`;
			}
		}
	}
	return null;
}

function pathHash(repoPath: string): string {
	return createHash("sha256").update(repoPath).digest("hex").slice(0, HASH_LENGTH);
}

// ─── Public API ───────────────────────────────────────────────────────────────

/**
 * Derive a stable repo identifier from a repo path.
 *
 * Priority:
 *   1. git remote origin URL → `host-owner-repo` (e.g. `github.com-foo-bar`)
 *   2. Fallback → `basename-<sha256[0:8]>` (e.g. `myrepo-a1b2c3d4`)
 *
 * P3: accepts optional injected `GitDeps` for testing.
 *
 * @returns Tagged-union `{ ok: true, repoId }` (P4). Never `ok: false` —
 *          the fallback always produces a valid id.
 */
export function deriveRepoId(
	repoPath: string,
	deps: GitDeps = nodeGitDeps(),
): { ok: boolean; repoId: string; error?: string } {
	const remoteUrl = deps.getRemoteOriginUrl(repoPath);
	if (remoteUrl !== null) {
		const parsed = parseRemoteUrl(remoteUrl);
		if (parsed !== null) {
			return { ok: true, repoId: parsed };
		}
	}

	// Fallback: basename + path hash
	const name = basename(repoPath);
	const hash = pathHash(repoPath);
	return { ok: true, repoId: `${name}-${hash}` };
}
