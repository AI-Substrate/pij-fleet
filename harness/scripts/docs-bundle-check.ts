#!/usr/bin/env tsx
/**
 * PRD 009 req-04 (plan 239): a bundled page and its file are one source.
 *
 * Asks the checkout's built pij-rs for every page it bundles (`docs list
 * --json` names each page's source file) and compares what `docs get` prints
 * with that file, byte for byte. Fails naming a page that differs, a
 * `docs/cli/**.md` file that is not bundled, and a bundled page whose file is
 * missing. The binary is read, never rebuilt: an edited page fails until
 * pij-rs is rebuilt.
 */

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

export const CLI_PAGES_DIR = "docs/cli";

export interface Run {
	readonly code: number;
	readonly stdout: string;
}

export interface CheckIo {
	/** Run the binary with `args`. */
	readonly run: (args: readonly string[]) => Run;
	/** A repo-relative file's text, or undefined when it is missing. */
	readonly readFile: (path: string) => string | undefined;
	/** Every `docs/cli/**.md`, repo-relative. */
	readonly cliFiles: () => readonly string[];
}

export interface CheckReport {
	readonly ok: boolean;
	readonly checked: number;
	readonly problems: readonly string[];
}

function data(run: Run, what: string): Record<string, unknown> | string {
	if (run.code !== 0) return `${what} exited ${run.code}: ${run.stdout.trim()}`;
	try {
		const envelope = JSON.parse(run.stdout) as { ok?: unknown; data?: unknown };
		if (envelope.ok === true && typeof envelope.data === "object" && envelope.data !== null)
			return envelope.data as Record<string, unknown>;
	} catch {
		// reported below
	}
	return `${what} printed no ok JSON envelope: ${run.stdout.trim().slice(0, 200)}`;
}

export function check(io: CheckIo): CheckReport {
	const listed = data(io.run(["docs", "list", "--json"]), "pij-rs docs list --json");
	if (typeof listed === "string") return { ok: false, checked: 0, problems: [listed] };
	const pages = Array.isArray(listed.pages) ? listed.pages : [];
	const problems: string[] = [];
	const sources = new Set<string>();
	for (const entry of pages as Array<{ page?: unknown; source?: unknown }>) {
		const page = typeof entry.page === "string" ? entry.page : undefined;
		const source = typeof entry.source === "string" ? entry.source : undefined;
		if (page === undefined || source === undefined) {
			problems.push(`a listed page has no page id or source: ${JSON.stringify(entry)}`);
			continue;
		}
		sources.add(source);
		const file = io.readFile(source);
		if (file === undefined) {
			problems.push(`${page}: its source ${source} is missing`);
			continue;
		}
		const got = data(io.run(["docs", "get", page, "--json"]), `pij-rs docs get ${page} --json`);
		if (typeof got === "string") problems.push(`${page}: ${got}`);
		else if (got.text !== file)
			problems.push(`${page}: the bundled page differs from ${source}; rebuild pij-rs`);
	}
	for (const file of io.cliFiles())
		if (!sources.has(file))
			problems.push(`${file} is not bundled: add it to PAGES in crates/cli/src/docs.rs`);
	if (pages.length === 0) problems.push("pij-rs bundles no pages; refusing to pass on nothing");
	return { ok: problems.length === 0, checked: pages.length, problems };
}

function walk(root: string, dir: string, out: string[]): void {
	for (const entry of readdirSync(join(root, dir), { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) walk(root, path, out);
		else if (entry.name.endsWith(".md")) out.push(path);
	}
}

/** `PIJ_RS_BIN`, else the checkout's built debug pij-rs. */
export function binary(env: NodeJS.ProcessEnv, root: string): string {
	if (env.PIJ_RS_BIN) return env.PIJ_RS_BIN;
	const target = execFileSync("bash", [join(root, "harness/scripts/cargo-target.sh")], {
		cwd: root,
		encoding: "utf8",
	}).trim();
	return join(target, "debug", "pij-rs");
}

function main(): void {
	const root = process.cwd();
	const bin = binary(process.env, root);
	if (!existsSync(bin)) {
		process.stderr.write(
			`docs-bundle-check: no built pij-rs at ${bin}, so no page was checked. Build it: ` +
				'PIJ_SHARED_TARGET="$(bash harness/scripts/cargo-target.sh)" just shared-build -- build --locked -p pij-cli --bin pij-rs\n',
		);
		process.exitCode = 2;
		return;
	}
	const report = check({
		run: (args) => {
			try {
				const stdout = execFileSync(bin, args, {
					encoding: "utf8",
					// The docs verbs contact no daemon; a closed port proves it.
					env: { ...process.env, PIJ_RS_ADDR: "127.0.0.1:9" },
				});
				return { code: 0, stdout };
			} catch (error) {
				const failed = error as { status?: number; stdout?: string };
				return { code: failed.status ?? 1, stdout: failed.stdout ?? String(error) };
			}
		},
		readFile: (path) => {
			try {
				return readFileSync(join(root, path), "utf8");
			} catch {
				return undefined;
			}
		},
		cliFiles: () => {
			const out: string[] = [];
			if (existsSync(join(root, CLI_PAGES_DIR))) walk(root, CLI_PAGES_DIR, out);
			return out.sort();
		},
	});
	if (report.ok) {
		process.stdout.write(
			`docs-bundle-check: ${report.checked} bundled page(s) in ${bin} match their files.\n`,
		);
		return;
	}
	process.stderr.write(`docs-bundle-check: ${report.problems.length} problem(s) in ${bin}:\n`);
	for (const problem of report.problems) process.stderr.write(`  ${problem}\n`);
	process.exitCode = 1;
}

if (process.argv[1]?.endsWith("docs-bundle-check.ts")) main();
