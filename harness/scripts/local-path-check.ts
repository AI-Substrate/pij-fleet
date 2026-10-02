#!/usr/bin/env tsx

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { extname, resolve } from "node:path";

export interface LocalPathFinding {
	readonly file: string;
	readonly line: number;
	readonly kind: string;
	readonly text: string;
}

const ALLOW_MARKER = "local-path-check: allow";
const ROOT_FILES = new Set(["justfile", "package.json", "biome.json", "tsconfig.json"]);
const OPERATIONAL_PREFIXES = [
	".github/",
	".harness/extensions/",
	".omp/extensions/",
	"agents/",
	"harness/",
	"skills/",
];
const OPERATIONAL_EXTENSIONS = new Set([
	".cjs",
	".js",
	".json",
	".md",
	".mjs",
	".py",
	".sh",
	".ts",
	".yaml",
	".yml",
]);
const TEST_FILE = /\.(?:test|spec)\.[cm]?[jt]s$/;
const EXCLUDED_SEGMENTS = ["/__snapshots__/", "/fixtures/", "/node_modules/"];
const EXCLUDED_BASENAMES = new Set([".minih-source.json"]);

const LOCAL_HOME_PATTERNS: ReadonlyArray<{ readonly kind: string; readonly pattern: RegExp }> = [
	{ kind: "WSL Windows user home", pattern: /\/mnt\/[A-Za-z]\/Users\/[A-Za-z0-9._-]+\//i },
	{
		kind: "Windows user home",
		pattern: /[A-Za-z]:[\\/]+Users[\\/]+[A-Za-z0-9._-]+[\\/]+/i,
	},
	{ kind: "macOS user home", pattern: /\/Users\/[A-Za-z0-9._-]+\// },
	{ kind: "Linux user home", pattern: /\/home\/[A-Za-z0-9._-]+\// },
];

function basename(path: string): string {
	return path.slice(path.lastIndexOf("/") + 1);
}

function isCommentOnly(line: string): boolean {
	const trimmed = line.trimStart();
	return (
		trimmed.startsWith("#") ||
		trimmed.startsWith("//") ||
		trimmed.startsWith("/*") ||
		trimmed.startsWith("*") ||
		trimmed.startsWith("<!--")
	);
}

export function shouldScanFile(path: string): boolean {
	if (ROOT_FILES.has(path)) return true;
	if (!OPERATIONAL_PREFIXES.some((prefix) => path.startsWith(prefix))) return false;
	if (TEST_FILE.test(path)) return false;
	if (EXCLUDED_SEGMENTS.some((segment) => path.includes(segment))) return false;
	if (EXCLUDED_BASENAMES.has(basename(path))) return false;
	return OPERATIONAL_EXTENSIONS.has(extname(path));
}

export function scanText(file: string, text: string): LocalPathFinding[] {
	const lines = text.split(/\r?\n/);
	const findings: LocalPathFinding[] = [];

	for (let index = 0; index < lines.length; index += 1) {
		const line = lines[index] ?? "";
		const previous = index > 0 ? (lines[index - 1] ?? "") : "";
		if (isCommentOnly(line) || line.includes(ALLOW_MARKER) || previous.includes(ALLOW_MARKER)) {
			continue;
		}
		for (const candidate of LOCAL_HOME_PATTERNS) {
			if (!candidate.pattern.test(line)) continue;
			findings.push({
				file,
				line: index + 1,
				kind: candidate.kind,
				text: line.trim(),
			});
			break;
		}
	}

	return findings;
}

function trackedFiles(root: string): string[] {
	return execFileSync(
		"git",
		["-C", root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
		{
			encoding: "utf8",
		},
	)
		.split("\0")
		.filter((path) => path.length > 0);
}

export function scanRepository(root: string): LocalPathFinding[] {
	const findings: LocalPathFinding[] = [];
	for (const file of trackedFiles(root).filter(shouldScanFile)) {
		findings.push(...scanText(file, readFileSync(resolve(root, file), "utf8")));
	}
	return findings;
}

function main(): void {
	const findings = scanRepository(process.cwd());
	if (findings.length === 0) {
		process.stdout.write(
			"local-path-check: no user-specific absolute home paths in operational files.\n",
		);
		return;
	}

	process.stderr.write(
		`local-path-check: found ${findings.length} user-specific absolute home path(s):\n`,
	);
	for (const finding of findings) {
		process.stderr.write(`  ${finding.file}:${finding.line} [${finding.kind}] ${finding.text}\n`);
	}
	process.stderr.write(
		"Use a repository-relative path, $HOME/homedir(), or an injected path. " +
			`For an intentional regression fixture, add "${ALLOW_MARKER}" on the same or preceding line.\n`,
	);
	process.exitCode = 1;
}

if (process.argv[1]?.endsWith("local-path-check.ts")) main();
