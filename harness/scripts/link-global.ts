#!/usr/bin/env tsx
// Machine-wide pij link policy.
//
// `just link` links only the `pij` extension into OMP, plus pij's curated OMP
// MCP and model configuration, and the native Copilot extension. OMP deliberately
// loads no other local extensions. `--check-only` exposes the linked-worktree
// guard without mutating any home.

import { randomUUID } from "node:crypto";
import {
	closeSync,
	existsSync,
	fsyncSync,
	lstatSync,
	mkdirSync,
	openSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	renameSync,
	statSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

type Verdict = "linked" | "already" | "removed" | "missing" | "skipped";

export interface RunLinkGlobalOptions {
	readonly pijRoot: string;
	readonly home: string;
	readonly copilotHome?: string;
	readonly fsync?: (fd: number) => void;
	readonly args: readonly string[];
	readonly stdout: (line: string) => void;
	readonly stderr: (line: string) => void;
}

function canonicalRootFromGitFile(pijRoot: string): string | undefined {
	const gitFile = join(pijRoot, ".git");
	const stat = lstatSync(gitFile, { throwIfNoEntry: false });
	if (!stat?.isFile()) return undefined;
	let raw: string;
	try {
		raw = readFileSync(gitFile, "utf8");
	} catch {
		return pijRoot;
	}
	const match = /^gitdir:\s*(.+)\s*$/m.exec(raw);
	if (!match?.[1]) return pijRoot;
	const gitDir = isAbsolute(match[1]) ? resolve(match[1]) : resolve(pijRoot, match[1]);
	const marker = `${sep}.git${sep}worktrees${sep}`;
	const markerAt = gitDir.lastIndexOf(marker);
	return markerAt === -1 ? pijRoot : gitDir.slice(0, markerAt);
}

function absoluteLinkTarget(linkPath: string, target: string): string {
	return isAbsolute(target) ? resolve(target) : resolve(dirname(linkPath), target);
}

/** `.pi` stays recognised so links made before pi was retired (which reached the
 *  OMP extension through `<repo>/.pi/extensions/pij`) migrate instead of being
 *  refused as foreign. */
const PIJ_EXTENSION_NAMESPACES: Readonly<Record<string, readonly string[]>> = {
	".omp": [".omp", ".pi"],
	".copilot": [".copilot"],
};

function isPijOwnedExtensionTarget(
	linkPath: string,
	target: string,
	name: string,
	namespace: ".omp" | ".copilot",
): boolean {
	const absolute = absoluteLinkTarget(linkPath, target);
	if (basename(absolute) !== name) return false;
	if (basename(dirname(absolute)) !== "extensions") return false;
	if (!PIJ_EXTENSION_NAMESPACES[namespace]?.includes(basename(dirname(dirname(absolute)))))
		return false;
	try {
		const packageJson = JSON.parse(
			readFileSync(resolve(absolute, "..", "..", "..", "package.json"), "utf8"),
		) as unknown;
		return (
			typeof packageJson === "object" &&
			packageJson !== null &&
			"name" in packageJson &&
			(packageJson as { readonly name?: unknown }).name === "pij"
		);
	} catch {
		return false;
	}
}

function ensureDirectory(path: string): void {
	if (!existsSync(path)) mkdirSync(path, { recursive: true });
}

function linkExtension(
	sourceRoot: string,
	targetRoot: string,
	name: string,
	stderr: (line: string) => void,
	namespace: ".omp" | ".copilot",
): Verdict {
	const source = join(sourceRoot, name);
	const target = join(targetRoot, name);
	const stat = lstatSync(target, { throwIfNoEntry: false });
	if (!stat) {
		symlinkSync(source, target);
		return "linked";
	}
	if (!stat.isSymbolicLink()) {
		stderr(`skip ${target}: real directory or file; refusing to clobber`);
		return "skipped";
	}
	const current = readlinkSync(target);
	if (absoluteLinkTarget(target, current) === resolve(source)) return "already";
	if (!isPijOwnedExtensionTarget(target, current, name, namespace)) {
		stderr(`skip ${target}: refusing to replace foreign symlink -> ${current}`);
		return "skipped";
	}
	unlinkSync(target);
	symlinkSync(source, target);
	return "linked";
}

function removeExtension(
	targetRoot: string,
	name: string,
	stderr: (line: string) => void,
	namespace: ".omp" | ".copilot",
): Verdict {
	const target = join(targetRoot, name);
	const stat = lstatSync(target, { throwIfNoEntry: false });
	if (!stat) return "missing";
	if (!stat.isSymbolicLink()) {
		stderr(`skip ${target}: real directory or file; refusing to clobber`);
		return "skipped";
	}
	const current = readlinkSync(target);
	if (!isPijOwnedExtensionTarget(target, current, name, namespace)) {
		stderr(`skip ${target}: refusing to remove foreign symlink -> ${current}`);
		return "skipped";
	}
	unlinkSync(target);
	return "removed";
}

function enforceOmpPijOnly(
	sourceRoot: string,
	targetRoot: string,
	removeMode: boolean,
	stdout: (line: string) => void,
	stderr: (line: string) => void,
): { changed: number; skipped: number } {
	let changed = 0;
	let skipped = 0;
	ensureDirectory(targetRoot);
	for (const name of readdirSync(targetRoot).sort()) {
		if (name === "pij") continue;
		const verdict = removeExtension(targetRoot, name, stderr, ".omp");
		if (verdict === "removed") {
			stdout(`✗ omp/${name} (pij-only policy)`);
			changed++;
		} else if (verdict === "skipped") {
			skipped++;
		}
	}
	const verdict = removeMode
		? removeExtension(targetRoot, "pij", stderr, ".omp")
		: linkExtension(sourceRoot, targetRoot, "pij", stderr, ".omp");
	if (verdict === "linked" || verdict === "removed") {
		stdout(`${verdict === "linked" ? "→" : "✗"} omp/pij`);
		changed++;
	} else if (verdict === "already") {
		stdout("= omp/pij (already linked)");
	} else if (verdict === "skipped") {
		skipped++;
	}
	return { changed, skipped };
}

function manageOmpSharedFile(
	home: string,
	name: "mcp.json" | "models.yml",
	source: string,
	sourceLabel: string,
	removeMode: boolean,
	stdout: (line: string) => void,
	stderr: (line: string) => void,
	previousSources: readonly string[] = [],
): { changed: number; skipped: number } {
	const target = join(home, ".omp", "agent", name);
	const stat = lstatSync(target, { throwIfNoEntry: false });
	if (!stat) {
		if (removeMode) return { changed: 0, skipped: 0 };
		ensureDirectory(dirname(target));
		symlinkSync(source, target);
		stdout(`→ omp/${name} -> ${sourceLabel}`);
		return { changed: 1, skipped: 0 };
	}
	if (!stat.isSymbolicLink()) {
		stderr(`skip ${target}: real file; refusing to clobber`);
		return { changed: 0, skipped: 1 };
	}
	const current = readlinkSync(target);
	const currentAbsolute = absoluteLinkTarget(target, current);
	if (
		currentAbsolute !== resolve(source) &&
		previousSources.some((previous) => resolve(previous) === currentAbsolute)
	) {
		// pij managed this link before, at a location it no longer uses.
		unlinkSync(target);
		if (removeMode) {
			stdout(`✗ omp/${name}`);
			return { changed: 1, skipped: 0 };
		}
		symlinkSync(source, target);
		stdout(`→ omp/${name} -> ${sourceLabel} (migrated from ${current})`);
		return { changed: 1, skipped: 0 };
	}
	if (currentAbsolute !== resolve(source)) {
		// A symlink whose target does not exist protects nothing: the common case
		// here is a link into a deleted worktree, which leaves `just link` — and
		// therefore `update-pi`/`update-omp` — permanently failing on state that
		// resolves to nothing. Reclaiming it can only ever replace a dead path;
		// a link to a file that still exists is still refused.
		if (existsSync(target)) {
			stderr(`skip ${target}: refusing to replace foreign symlink -> ${current}`);
			return { changed: 0, skipped: 1 };
		}
		stderr(`= ${target}: reclaiming dangling symlink -> ${current}`);
		unlinkSync(target);
		if (removeMode) return { changed: 1, skipped: 0 };
		symlinkSync(source, target);
		stdout(`→ omp/${name} -> ${sourceLabel}`);
		return { changed: 1, skipped: 0 };
	}
	if (!removeMode) {
		stdout(`= omp/${name} (already linked)`);
		return { changed: 0, skipped: 0 };
	}
	unlinkSync(target);
	stdout(`✗ omp/${name}`);
	return { changed: 1, skipped: 0 };
}

function doctorOmp(options: RunLinkGlobalOptions, sourceRoot: string): number {
	const ompExtensions = join(options.home, ".omp", "agent", "extensions");
	let names: string[] = [];
	try {
		names = readdirSync(ompExtensions).sort();
	} catch {
		options.stderr(`OMP extensions directory missing: ${ompExtensions}`);
		return 1;
	}
	if (names.length !== 1 || names[0] !== "pij") {
		options.stderr(
			`OMP extension policy violation: expected only pij, found ${names.join(", ") || "none"}`,
		);
		return 1;
	}
	const pijLink = join(ompExtensions, "pij");
	const mcpLink = join(options.home, ".omp", "agent", "mcp.json");
	const modelsLink = join(options.home, ".omp", "agent", "models.yml");
	const expectedPij = join(sourceRoot, "pij");
	const expectedMcp = join(options.pijRoot, ".omp", "mcp.json");
	const expectedModels = join(options.pijRoot, ".omp", "models.yml");
	if (
		!lstatSync(pijLink, { throwIfNoEntry: false })?.isSymbolicLink() ||
		absoluteLinkTarget(pijLink, readlinkSync(pijLink)) !== resolve(expectedPij)
	) {
		options.stderr(`OMP pij link mismatch: expected ${expectedPij}`);
		return 1;
	}
	if (
		!lstatSync(mcpLink, { throwIfNoEntry: false })?.isSymbolicLink() ||
		absoluteLinkTarget(mcpLink, readlinkSync(mcpLink)) !== resolve(expectedMcp)
	) {
		options.stderr(`OMP MCP link mismatch: expected ${expectedMcp}`);
		return 1;
	}
	if (
		!lstatSync(modelsLink, { throwIfNoEntry: false })?.isSymbolicLink() ||
		absoluteLinkTarget(modelsLink, readlinkSync(modelsLink)) !== resolve(expectedModels)
	) {
		options.stderr(`OMP models link mismatch: expected ${expectedModels}`);
		return 1;
	}
	options.stdout("✓ OMP policy: pij-only extension + curated MCP and model config");
	return 0;
}

/** Filesystem operations shared by canonical setup and isolated smoke fixtures.
 * The executable entry point still enforces the canonical-checkout guard.
 */
export function manageCopilotExtension(options: RunLinkGlobalOptions): {
	changed: number;
	skipped: number;
} {
	const home = resolve(options.copilotHome || join(options.home, ".copilot"));
	const extensions = join(home, "extensions");
	const sourceRoot = join(options.pijRoot, ".copilot", "extensions");
	const source = join(sourceRoot, "pij");
	const target = join(extensions, "pij");
	const settingsPath = join(home, "settings.json");
	const remove = options.args.includes("--remove");
	const doctor = options.args.includes("--doctor-copilot");
	const sync = options.fsync ?? fsyncSync;
	let changed = 0;
	try {
		// These are preservation brakes: removing them can only broaden writes.
		for (const directory of [home, extensions]) {
			const stat = lstatSync(directory, { throwIfNoEntry: false });
			if (stat && (!stat.isDirectory() || stat.isSymbolicLink())) {
				options.stderr(`Copilot: refusing non-directory or symlinked path ${directory}`);
				return { changed, skipped: 1 };
			}
		}
		if (remove) {
			const verdict = removeExtension(extensions, "pij", options.stderr, ".copilot");
			options.stdout(`Copilot extension: ${verdict}; settings preserved`);
			return { changed: verdict === "removed" ? 1 : 0, skipped: verdict === "skipped" ? 1 : 0 };
		}
		for (const name of ["extension.mjs", "store.mjs"]) {
			if (!lstatSync(join(source, name), { throwIfNoEntry: false })?.isFile()) {
				options.stderr(
					`Copilot source missing: ${join(source, name)}; use the complete reviewed checkout`,
				);
				return { changed, skipped: 1 };
			}
		}
		const targetStat = lstatSync(target, { throwIfNoEntry: false });
		if (
			targetStat &&
			(!targetStat.isSymbolicLink() ||
				!isPijOwnedExtensionTarget(target, readlinkSync(target), "pij", ".copilot"))
		) {
			options.stderr(`Copilot: refusing foreign symlink or real file/directory ${target}`);
			return { changed, skipped: 1 };
		}
		const settingsStat = lstatSync(settingsPath, { throwIfNoEntry: false });
		if (settingsStat && (!settingsStat.isFile() || settingsStat.nlink !== 1)) {
			options.stderr(
				`Copilot: refusing non-regular, symlinked or multiply-linked settings ${settingsPath}`,
			);
			return { changed, skipped: 1 };
		}
		const settings: unknown = settingsStat ? JSON.parse(readFileSync(settingsPath, "utf8")) : {};
		if (settings === null || typeof settings !== "object" || Array.isArray(settings)) {
			options.stderr(
				`Copilot settings must be a JSON object: ${settingsPath}; refusing to overwrite`,
			);
			return { changed, skipped: 1 };
		}
		const enabled = "experimental" in settings && settings.experimental === true;
		if (doctor) {
			if (!targetStat || absoluteLinkTarget(target, readlinkSync(target)) !== resolve(source)) {
				options.stderr(
					`Copilot extension link mismatch: expected ${source}; run just link from the canonical checkout`,
				);
				return { changed, skipped: 1 };
			}
			if (!enabled) {
				options.stderr(
					`Copilot experimental is disabled/missing in ${settingsPath}; run just link to opt in. --no-experimental remains an explicit per-session opt-out.`,
				);
				return { changed, skipped: 1 };
			}
			options.stdout(
				`Copilot native extension ready: ${target}; experimental enabled (new CLI sessions only)`,
			);
			return { changed, skipped: 0 };
		}
		ensureDirectory(extensions);
		if (!enabled) {
			const temporary = join(home, `.settings-${randomUUID()}.tmp`);
			try {
				const fd = openSync(temporary, "wx", settingsStat ? settingsStat.mode & 0o777 : 0o600);
				try {
					writeFileSync(fd, `${JSON.stringify({ ...settings, experimental: true }, null, "\t")}\n`);
					sync(fd);
				} finally {
					closeSync(fd);
				}
				renameSync(temporary, settingsPath);
				changed++;
				let directory: number | undefined;
				try {
					directory = openSync(home, "r");
					sync(directory);
				} catch (error) {
					// Some platforms/filesystems do not support syncing directory descriptors.
					if (
						!["EINVAL", "ENOTSUP", "EISDIR"].includes((error as NodeJS.ErrnoException).code ?? "")
					)
						throw error;
				} finally {
					if (directory !== undefined) closeSync(directory);
				}
			} finally {
				if (lstatSync(temporary, { throwIfNoEntry: false })) unlinkSync(temporary);
			}
		}
		const verdict = linkExtension(sourceRoot, extensions, "pij", options.stderr, ".copilot");
		if (verdict === "linked") changed++;
		options.stdout(`Copilot extension: ${verdict}; experimental enabled`);
		return { changed, skipped: verdict === "skipped" ? 1 : 0 };
	} catch (error) {
		options.stderr(
			`Copilot setup failed${changed ? " after applying changes" : ""}: ${error instanceof Error ? error.message : String(error)}`,
		);
		return { changed, skipped: 1 };
	}
}

export function runLinkGlobal(options: RunLinkGlobalOptions): number {
	const worktreeCanonicalRoot = canonicalRootFromGitFile(options.pijRoot);
	if (worktreeCanonicalRoot !== undefined) {
		options.stderr(
			`refusing machine-wide links from linked worktree ${options.pijRoot}; run \`just link\` from ${worktreeCanonicalRoot}`,
		);
		return 1;
	}
	if (options.args.includes("--check-only")) return 0;
	const sourceRoot = join(options.pijRoot, ".omp", "extensions");
	if (options.args.includes("--doctor-omp")) return doctorOmp(options, sourceRoot);
	if (options.args.includes("--doctor-copilot")) return manageCopilotExtension(options).skipped;
	const removeMode = options.args.includes("--remove");
	if (!statSync(join(sourceRoot, "pij"), { throwIfNoEntry: false })?.isDirectory()) {
		options.stderr(`no pij extension at ${join(sourceRoot, "pij")}`);
		return 1;
	}

	const ompTargetRoot = join(options.home, ".omp", "agent", "extensions");
	let changed = 0;
	let skipped = 0;
	const omp = enforceOmpPijOnly(
		sourceRoot,
		ompTargetRoot,
		removeMode,
		options.stdout,
		options.stderr,
	);
	changed += omp.changed;
	skipped += omp.skipped;
	for (const [name, source, sourceLabel, previous] of [
		[
			"mcp.json",
			join(options.pijRoot, ".omp", "mcp.json"),
			"pij/.omp/mcp.json",
			[join(options.home, ".pi", "agent", "mcp.json")],
		],
		["models.yml", join(options.pijRoot, ".omp", "models.yml"), "pij/.omp/models.yml", []],
	] as const) {
		const shared = manageOmpSharedFile(
			options.home,
			name,
			source,
			sourceLabel,
			removeMode,
			options.stdout,
			options.stderr,
			previous,
		);
		changed += shared.changed;
		skipped += shared.skipped;
	}
	const copilot = manageCopilotExtension(options);
	changed += copilot.changed;
	skipped += copilot.skipped;
	if (changed === 0 && skipped === 0) {
		options.stdout(removeMode ? "nothing to remove" : "everything already linked");
	}
	return skipped > 0 ? 1 : 0;
}

const directEntry = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (directEntry === import.meta.url) {
	process.exit(
		runLinkGlobal({
			pijRoot: resolve(import.meta.dirname, "..", ".."),
			home: homedir(),
			copilotHome: process.env.COPILOT_HOME,
			args: process.argv.slice(2),
			stdout: console.log,
			stderr: console.error,
		}),
	);
}
