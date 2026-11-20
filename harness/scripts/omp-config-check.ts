// F219: OMP rewrites `git worktree add` (and `github pr_checkout`) into its own
// worktree command, which by default clones the whole checkout copy-on-write,
// ignored files included: target/ and node_modules came along into every lane.
// The committed project layer `.omp/config.yml` turns that off. This check
// fails if it stops doing so:
//
// - statically, everywhere (CI has no omp): the file sets `worktree:` /
//   `clone: false`;
// - live, when omp is on PATH: `omp config get worktree.clone --json`, run from
//   the repo root, answers false. OMP reads the project layer from the session's
//   start directory (and an explicit `omp worktree add`'s cwd), so the root is
//   where seats start. `omp config get` may run OMP's own settings
//   initialization or migrations, so this half is not provably read-only on
//   OMP's global state; the probes so far left ~/.omp/agent/config.yml unchanged.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

export const CONFIG = ".omp/config.yml";

/** OMP's answer for `worktree.clone` from `root`, or undefined without omp. */
export type OmpClone = (root: string) => boolean | undefined;

/**
 * `clone: false` as a direct child of a top-level `worktree:` block (at the
 * block's first child indent, never deeper), or in a top-level inline mapping
 * `worktree: {clone: false}` (R260-002), comments ignored; the last such
 * `clone` wins, as in YAML.
 */
export function setsCloneFalse(yaml: string): boolean {
	let inWorktree = false;
	let childIndent: number | undefined;
	let clone: string | undefined;
	for (const raw of yaml.split("\n")) {
		const line = raw.replace(/\s+#.*$/, "").replace(/^#.*$/, "");
		if (line.trim() === "") continue;
		const indent = line.length - line.trimStart().length;
		if (indent === 0) {
			inWorktree = /^worktree:\s*$/.test(line);
			childIndent = undefined;
			const inline = /^worktree:\s*\{(.*)\}\s*$/.exec(line)?.[1];
			for (const entry of inline?.split(",") ?? []) {
				const value = /^\s*clone:\s*(\S+?)\s*$/.exec(entry)?.[1];
				if (value !== undefined) clone = value;
			}
			continue;
		}
		if (!inWorktree) continue;
		childIndent ??= indent;
		if (indent !== childIndent) continue;
		const value = /^clone:\s*(\S+)\s*$/.exec(line.trimStart())?.[1];
		if (value !== undefined) clone = value;
	}
	return clone === "false";
}

export const ompClone: OmpClone = (root) => {
	let out: string;
	try {
		out = execFileSync("omp", ["config", "get", "worktree.clone", "--json"], {
			cwd: root,
			encoding: "utf8",
			stdio: ["ignore", "pipe", "pipe"],
		});
	} catch (error) {
		if (error instanceof Error && "code" in error && error.code === "ENOENT") return undefined;
		throw error;
	}
	const value: unknown = JSON.parse(out).value;
	if (typeof value !== "boolean") throw new Error(`omp config get worktree.clone: ${out}`);
	return value;
};

/** Findings for the repo at `root`; none means new worktrees are plain checkouts. */
export function ompConfigFindings(root: string, omp: OmpClone = ompClone): string[] {
	const findings: string[] = [];
	const path = join(root, CONFIG);
	if (!existsSync(path)) findings.push(`${CONFIG} is missing; it must set worktree.clone: false`);
	else if (!setsCloneFalse(readFileSync(path, "utf8")))
		findings.push(`${CONFIG} does not set worktree:\n  clone: false`);
	const live = omp(root);
	if (live === true)
		findings.push(
			"omp config get worktree.clone answers true from the repo root: OMP would clone the checkout, target/ and node_modules included, into every new worktree",
		);
	return findings;
}

function main(): void {
	const root = resolve(import.meta.dirname, "../..");
	const findings = ompConfigFindings(root);
	for (const finding of findings) console.error(`omp-config-check: ${finding}`);
	if (findings.length > 0) {
		process.exitCode = 1;
		return;
	}
	console.log("omp-config-check: new worktrees are plain checkouts (worktree.clone false)");
}

if (process.argv[1]?.endsWith("omp-config-check.ts")) main();
