#!/usr/bin/env tsx

import { spawnSync } from "node:child_process";
import { accessSync, constants, readdirSync, statSync } from "node:fs";
import { delimiter, isAbsolute, join, resolve } from "node:path";

/** Trees whose deterministic documents the sensor checks (repo-relative). */
export const DDOCS_ROOTS: readonly string[] = ["docs/project", "skills/pij/references/community"];

const SKIPPED_DIRS = new Set(["node_modules", ".git"]);

export type DdocsResolution =
	| { readonly ok: true; readonly path: string; readonly via: "PIJ_DDOCS" | "PATH" }
	| { readonly ok: false; readonly reason: string };

export interface DocVerdict {
	readonly file: string;
	readonly ok: boolean;
	readonly detail: string;
}

/** Ruling C10: PIJ_DDOCS, else PATH. A set but unusable PIJ_DDOCS refuses; it never falls back. */
export function resolveDdocs(
	env: Readonly<Record<string, string | undefined>>,
	isExecutable: (path: string) => boolean,
): DdocsResolution {
	const pinned = env.PIJ_DDOCS;
	if (pinned !== undefined && pinned !== "") {
		if (!isAbsolute(pinned)) {
			return { ok: false, reason: `PIJ_DDOCS must be an absolute path, got "${pinned}"` };
		}
		return isExecutable(pinned)
			? { ok: true, path: pinned, via: "PIJ_DDOCS" }
			: { ok: false, reason: `PIJ_DDOCS=${pinned} is not an executable file` };
	}
	for (const dir of (env.PATH ?? "").split(delimiter)) {
		// A relative PATH entry would resolve against the caller's cwd; skip it.
		if (!isAbsolute(dir)) continue;
		const candidate = join(dir, "ddocs");
		if (isExecutable(candidate)) return { ok: true, path: candidate, via: "PATH" };
	}
	return { ok: false, reason: "ddocs is not on PATH and PIJ_DDOCS is unset" };
}

export type DirListing =
	| {
			readonly ok: true;
			readonly entries: ReadonlyArray<{ readonly name: string; readonly isDir: boolean }>;
	  }
	| { readonly ok: false; readonly error: string };

export interface Discovery {
	readonly documents: string[];
	readonly errors: ReadonlyArray<{ readonly path: string; readonly error: string }>;
}

/**
 * Every `*.dd.json` under the roots, repo-relative and sorted. A directory that
 * can't be read, including a root that doesn't exist, is an error, never an
 * empty listing: an unreadable tree must not pass as a clean one.
 */
export function findDocuments(
	roots: readonly string[],
	listDir: (dir: string) => DirListing,
): Discovery {
	const found: string[] = [];
	const errors: Array<{ path: string; error: string }> = [];
	const walk = (dir: string): void => {
		const listing = listDir(dir);
		if (!listing.ok) {
			errors.push({ path: dir, error: listing.error });
			return;
		}
		for (const entry of listing.entries) {
			const path = `${dir}/${entry.name}`;
			if (entry.isDir) {
				if (!SKIPPED_DIRS.has(entry.name)) walk(path);
			} else if (entry.name.endsWith(".dd.json")) {
				found.push(path);
			}
		}
	};
	for (const root of roots) walk(root);
	return { documents: found.sort(), errors };
}

interface Envelope {
	readonly status?: unknown;
	readonly data?: { readonly counts?: { readonly error?: unknown; readonly warn?: unknown } };
	readonly error?: { readonly code?: unknown; readonly message?: unknown };
}

function parseEnvelope(stdout: string): Envelope | null {
	try {
		const parsed: unknown = JSON.parse(stdout);
		return typeof parsed === "object" && parsed !== null ? (parsed as Envelope) : null;
	} catch {
		return null;
	}
}

/** `ddocs validate --json`: pass only on status ok with 0 errors and 0 warnings, and exit 0. */
export function judgeValidate(file: string, exitCode: number | null, stdout: string): DocVerdict {
	const envelope = parseEnvelope(stdout);
	if (envelope === null) {
		return { file, ok: false, detail: `validate printed no JSON envelope (exit ${exitCode})` };
	}
	const counts = envelope.data?.counts;
	if (exitCode === 0 && envelope.status === "ok" && counts?.error === 0 && counts?.warn === 0) {
		return { file, ok: true, detail: "validate ok, 0 error, 0 warn" };
	}
	const why =
		typeof envelope.error?.message === "string"
			? envelope.error.message
			: `status ${String(envelope.status)}, ${String(counts?.error)} error, ${String(counts?.warn)} warn`;
	return { file, ok: false, detail: `validate: ${why}` };
}

/** `ddocs build --check --json`: pass only when the committed .dd.md matches the render. */
export function judgeBuildCheck(file: string, exitCode: number | null, stdout: string): DocVerdict {
	const envelope = parseEnvelope(stdout);
	if (exitCode === 0 && envelope?.status === "ok") {
		return { file, ok: true, detail: "rendered .dd.md is current" };
	}
	const why =
		typeof envelope?.error?.message === "string"
			? envelope.error.message
			: `build --check failed (exit ${exitCode})`;
	return { file, ok: false, detail: `build --check: ${why}` };
}

function isExecutableFile(path: string): boolean {
	try {
		if (!statSync(path).isFile()) return false;
		accessSync(path, constants.X_OK);
		return true;
	} catch {
		return false;
	}
}

function listDirectory(root: string) {
	return (dir: string): DirListing => {
		try {
			const entries = readdirSync(resolve(root, dir), { withFileTypes: true }).map((entry) => ({
				name: entry.name,
				isDir: entry.isDirectory(),
			}));
			return { ok: true, entries };
		} catch (error) {
			return { ok: false, error: error instanceof Error ? error.message : String(error) };
		}
	};
}

function main(): void {
	const root = process.cwd();
	const warnIfUnavailable = process.argv.includes("--unavailable=warn");
	const ddocs = resolveDdocs(process.env, isExecutableFile);
	if (!ddocs.ok) {
		const message =
			`ddocs-check: UNAVAILABLE: ${ddocs.reason}. Nothing was validated. ` +
			"Set PIJ_DDOCS to the ddocs executable or put ddocs on PATH.";
		if (warnIfUnavailable) {
			process.stdout.write(`${message}\n`);
			if (process.env.GITHUB_ACTIONS === "true") {
				process.stdout.write(`::warning title=ddocs-check UNAVAILABLE::${message}\n`);
			}
			return;
		}
		process.stderr.write(`${message}\n`);
		process.exitCode = 2;
		return;
	}

	const { documents, errors } = findDocuments(DDOCS_ROOTS, listDirectory(root));
	if (errors.length > 0) {
		process.stderr.write(
			`ddocs-check: could not read ${errors.length === 1 ? "1 directory" : `${errors.length} directories`}, so nothing below ${errors.length === 1 ? "it" : "them"} was checked:\n`,
		);
		for (const failure of errors) {
			process.stderr.write(`  ${failure.path}: ${failure.error.replaceAll(`${root}/`, "")}\n`);
		}
		process.exitCode = 1;
		return;
	}
	if (documents.length === 0) {
		process.stderr.write(
			`ddocs-check: found no .dd.json under ${DDOCS_ROOTS.join(", ")}; refusing to pass on nothing.\n`,
		);
		process.exitCode = 1;
		return;
	}

	const verdicts: DocVerdict[] = [];
	for (const file of documents) {
		const validate = spawnSync(ddocs.path, ["validate", "--json", file], {
			cwd: root,
			encoding: "utf8",
		});
		verdicts.push(judgeValidate(file, validate.status, validate.stdout ?? ""));
		const build = spawnSync(ddocs.path, ["build", "--check", "--json", file], {
			cwd: root,
			encoding: "utf8",
		});
		verdicts.push(judgeBuildCheck(file, build.status, build.stdout ?? ""));
	}

	const failures = verdicts.filter((verdict) => !verdict.ok);
	if (failures.length === 0) {
		process.stdout.write(
			`ddocs-check: ${documents.length} document(s) validate clean and their .dd.md are current (ddocs via ${ddocs.via}).\n`,
		);
		return;
	}
	process.stderr.write(
		`ddocs-check: ${failures.length} failure(s) in ${documents.length} document(s):\n`,
	);
	for (const failure of failures) {
		process.stderr.write(`  ${failure.file}: ${failure.detail.replaceAll(`${root}/`, "")}\n`);
	}
	process.stderr.write(
		"Fix with ddocs (set/add), then `ddocs validate <file>` and `ddocs build <file>`; see skills/pij/references/community/README.md.\n",
	);
	process.exitCode = 1;
}

if (process.argv[1]?.endsWith("ddocs-check.ts")) main();
