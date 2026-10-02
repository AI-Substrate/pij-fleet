// Import-boundary sensor for core/platform (plan 054 — clone of the
// core/agents boundary sensor).
//
// The platform domain is pure core: dependency direction is
// `cli/daemon → adapters → core/platform`, never the reverse. This static scan
// flips that architectural rule from a review-tier concern into a computational
// one — every .ts file under `core/platform/**` (sources AND tests) is parsed
// for import specifiers, and any import of a daemon / telegram / tmux / grammy
// module, any specifier reaching `adapters/`, or any pi-runtime import
// (`@earendil-works/*` — core is pi-free, Pattern P2) fails the build.
// Allowed: ./siblings, ../types.js, ../memorable-id.js, other core/* modules,
// node builtins, and vitest in tests.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));

/** Forbidden module families — matched against import specifiers as path
 *  segments so a benign substring elsewhere can never false-positive. */
const FORBIDDEN = [
	// control-plane / transport layers (cloned from core/agents/boundary.test.ts)
	/(^|[./@])(daemon|telegram|tmux|grammy|@grammyjs)([./]|$)/,
	// fs adapters — core must never reach adapters/ (any depth: ../adapters, /adapters/)
	/(^|[./])adapters([./]|$)/,
	// pi runtime — core is pi-free (Pattern P2): no @earendil-works/* imports
	/^@earendil-works([./]|$)/,
];

/** Extra families for PRODUCTION modules only (review 001 F4): the platform
 *  core is no-fs/no-process, so filesystem and process builtins are illegal
 *  outside tests — `node:`-prefixed and bare forms alike. */
const FORBIDDEN_PRODUCTION = [
	/^(node:)?fs(\/promises)?$/,
	/^(node:)?child_process$/,
	/^(node:)?process$/,
];

/** Global `process` usage (env/argv/exit…) — an import-free escape hatch the
 *  specifier scan cannot see. Word boundary keeps processor/myprocess legal. */
const GLOBAL_PROCESS = /\bprocess\s*[.[]/;

function tsFiles(dir: string): string[] {
	const out: string[] = [];
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const full = join(dir, entry.name);
		if (entry.isDirectory()) {
			if (entry.name === "__fixtures__") continue;
			out.push(...tsFiles(full));
		} else if (entry.name.endsWith(".ts")) {
			out.push(full);
		}
	}
	return out;
}

/** Extract every import/export/require specifier from a source file. */
function importSpecifiers(source: string): string[] {
	const specs: string[] = [];
	const patterns = [
		/(?:import|export)\s[^;]*?\sfrom\s*["']([^"']+)["']/g, // import … from "x" / export … from "x"
		/(?:^|\n)\s*import\s*["']([^"']+)["']/g, //             side-effect import "x"
		/\bimport\(\s*["']([^"']+)["']\s*\)/g, //               dynamic import("x")
		/\brequire\(\s*["']([^"']+)["']\s*\)/g, //              require("x")
	];
	for (const re of patterns) {
		for (const m of source.matchAll(re)) {
			if (m[1]) specs.push(m[1]);
		}
	}
	return specs;
}

/** Every boundary violation in one source: base families always; fs/process
 *  imports and global `process` usage only when the file is production scope
 *  (non-.test.ts) — test files legally use node builtins. */
function scanViolations(source: string, production: boolean): string[] {
	const specs = importSpecifiers(source);
	const violations = specs.filter((s) => FORBIDDEN.some((re) => re.test(s)));
	if (production) {
		violations.push(...specs.filter((s) => FORBIDDEN_PRODUCTION.some((re) => re.test(s))));
		if (GLOBAL_PROCESS.test(source)) violations.push("global `process` usage");
	}
	return violations;
}

/** Stable label relative to core/platform. */
function labelOf(file: string): string {
	return file.slice(HERE.length + 1);
}

describe("import boundary — core/platform must not import daemon/telegram/tmux/grammy, adapters, or pi runtime", () => {
	const files = tsFiles(HERE);

	it("finds source files to scan (sanity)", () => {
		expect(files.length).toBeGreaterThan(0);
	});

	it("covers the platform contract files (types + ports + spine + time)", () => {
		const names = files.map(labelOf);
		// These pure modules form the platform contract surface; if a future
		// refactor narrows the scan glob they must not silently drop out.
		expect(names).toContain("types.ts");
		expect(names).toContain("ports.ts");
		expect(names).toContain("spine.ts");
		expect(names).toContain("time.ts");
	});

	it.each(
		files.map((f) => [labelOf(f), f]),
	)("%s imports only permitted modules", (_label, file) => {
		const production = !file.endsWith(".test.ts");
		const violations = scanViolations(readFileSync(file, "utf8"), production);
		expect(violations, `forbidden in ${file}: ${violations.join(", ")}`).toEqual([]);
	});
});

// ─── review 001 F4 — production scope: no fs, no process ───────────────────
// The stated boundary is pure-core/no-fs/no-process, but the base FORBIDDEN
// families allow node builtins everywhere. Production modules (non-.test.ts)
// get an extra forbidden family and a global-`process` source check; test
// files keep node builtins — this sensor itself imports node:fs.

describe("production-scope rules reject fs/process (review 001 F4)", () => {
	const flags = (source: string) => scanViolations(source, true);
	const testScopeFlags = (source: string) => scanViolations(source, false);

	it.each([
		['import { readFileSync } from "node:fs";', "node:fs"],
		['import { readFile } from "node:fs/promises";', "node:fs/promises"],
		['import { execSync } from "node:child_process";', "node:child_process"],
		['import process from "node:process";', "node:process"],
		['import { readFileSync } from "fs";', "fs (bare)"],
		['import { readFile } from "fs/promises";', "fs/promises (bare)"],
		['import { execSync } from "child_process";', "child_process (bare)"],
		['import process from "process";', "process (bare)"],
		['const fs = require("node:fs");', "require node:fs"],
		['await import("node:child_process");', "dynamic import child_process"],
	])("flags %s in a production module", (source) => {
		expect(flags(source).length).toBeGreaterThan(0);
	});

	it.each([
		["const v = process.env.HOME;", "member read"],
		['const v = process["env"];', "index read"],
		["exitWith(process.exitCode ?? 0);", "mid-expression"],
	])("flags global process usage in a production module: %s", (source) => {
		expect(flags(source).length).toBeGreaterThan(0);
	});

	it("does NOT flag identifiers that merely contain 'process'", () => {
		expect(flags("const processor = { run() {} }; processor.run();")).toEqual([]);
		expect(flags("myprocess.env;")).toEqual([]);
		expect(flags("reprocess.items[0];")).toEqual([]);
	});

	it("keeps node builtins legal in test scope (the sensor itself imports node:fs)", () => {
		expect(testScopeFlags('import { readFileSync } from "node:fs";')).toEqual([]);
		expect(testScopeFlags("const v = process.env.CI;")).toEqual([]);
	});

	it("still applies the base families in BOTH scopes", () => {
		// Assembled at runtime so this sensor's OWN source never carries a
		// scannable adapters specifier (the self-scan would flag it).
		const spec = ["..", "..", "adapters", "project-store.js"].join("/");
		const adapterImport = `import { FsProjectStore } from ${JSON.stringify(spec)};`;
		expect(flags(adapterImport).length).toBeGreaterThan(0);
		expect(testScopeFlags(adapterImport).length).toBeGreaterThan(0);
	});

	it("does not flag clean production imports", () => {
		expect(
			flags('import { err, ok } from "../types.js";\nimport { kebabSlug } from "./types.js";'),
		).toEqual([]);
	});
});
