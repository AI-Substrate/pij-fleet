// Lockfile source-host allowlist — the compensating control for npmjs-scoped
// host replacement (PR#25 adopt, dove ruling s052-npm-hardening-pr25).
//
// The two-layer supply-chain model:
//   1. Reads: `replace-registry-host=npmjs` routes npmjs registry reads through
//      the governed registry (PIJ_NPM_REGISTRY, default npmjs); git dependencies
//      would resolve from their origin, so each must be sanctioned explicitly.
//   2. Lockfile integrity: THIS static check asserts every `resolved` URL in
//      package-lock.json points at an allowed source. Unlike the old
//      `replace-registry-host=always` — which silently *redirected* a crafted
//      URL to the proxy at install time (mute, and only where the .npmrc
//      applies) — this catches a tampered lockfile ITSELF, loudly, at CI/review
//      time, everywhere. Tamper-detection, not tamper-absorption.
//
// Any `resolved` host outside the allowlist is a HARD failure.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** Registry tarball hosts a `resolved` URL may reference. npm's default
 *  registry host; npmjs-scoped replacement routes it through the proxy at
 *  install time, and the committed lockfile pins it here. */
export const ALLOWED_REGISTRY_HOSTS: readonly string[] = ["registry.npmjs.org"];

/** Sanctioned git dependency sources, matched on the base URL (the `#<commit>`
 *  suffix is ignored). A registry proxy cannot serve git, so a git dependency
 *  resolves from its origin and must be allowlisted here deliberately. There are
 *  none today: every dependency comes from the registry. */
export const ALLOWED_GIT_SOURCES: readonly string[] = [];

export interface LockfileViolation {
	readonly packagePath: string;
	readonly resolved: string;
	readonly host: string;
}

interface LockPackage {
	readonly resolved?: string;
}

/** Return every `resolved` entry whose source is NOT in the allowlist. Empty ⇒
 *  the lockfile is clean. Pure — takes parsed lock JSON, does no I/O. */
export function findDisallowedResolved(lockJson: unknown): LockfileViolation[] {
	const packages = (lockJson as { packages?: Record<string, LockPackage> } | null)?.packages ?? {};
	const violations: LockfileViolation[] = [];
	for (const [packagePath, entry] of Object.entries(packages)) {
		const resolved = entry?.resolved;
		if (!resolved) continue; // root/workspace/link entries carry no resolved URL
		if (resolved.startsWith("git+") || resolved.startsWith("git:")) {
			const base = resolved.split("#")[0] ?? resolved;
			if (!ALLOWED_GIT_SOURCES.includes(base)) {
				violations.push({ packagePath, resolved, host: base });
			}
			continue;
		}
		let host: string;
		try {
			host = new URL(resolved).host;
		} catch {
			violations.push({ packagePath, resolved, host: "(unparseable)" });
			continue;
		}
		if (!ALLOWED_REGISTRY_HOSTS.includes(host)) {
			violations.push({ packagePath, resolved, host });
		}
	}
	return violations;
}

/** Format a hard-fail message naming every offending entry. */
export function formatViolations(violations: readonly LockfileViolation[]): string {
	const lines = [
		`lockfile-allowlist: ${violations.length} disallowed resolved source(s) in package-lock.json.`,
		`Allowed: registry host ${ALLOWED_REGISTRY_HOSTS.join(", ")}; git source ${ALLOWED_GIT_SOURCES.join(", ")}.`,
		"A resolved URL outside the allowlist means the lockfile was tampered with or a new source was added without review:",
	];
	for (const v of violations) {
		lines.push(`  ✗ ${v.packagePath} → ${v.host}  (${v.resolved})`);
	}
	return lines.join("\n");
}

function main(): void {
	const lockPath = resolve(process.argv[2] ?? "package-lock.json");
	let lockJson: unknown;
	try {
		lockJson = JSON.parse(readFileSync(lockPath, "utf8"));
	} catch (error) {
		console.error(
			`lockfile-allowlist: could not read/parse ${lockPath}: ${error instanceof Error ? error.message : String(error)}`,
		);
		process.exit(2);
	}
	const violations = findDisallowedResolved(lockJson);
	if (violations.length > 0) {
		console.error(formatViolations(violations));
		process.exit(1);
	}
	const packages = (lockJson as { packages?: Record<string, LockPackage> }).packages ?? {};
	const checked = Object.values(packages).filter((entry) => entry?.resolved).length;
	console.log(`lockfile-allowlist: ok — ${checked} resolved source(s), all allowlisted.`);
}

// Run as CLI only (not when imported by tests).
if (import.meta.url === `file://${process.argv[1]}`) {
	main();
}
