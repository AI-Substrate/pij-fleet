import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { CONFIG, ompConfigFindings, setsCloneFalse } from "./omp-config-check.js";

// F219: the committed project layer keeps OMP from cloning the checkout (with
// target/ and node_modules) into every new worktree.

const ROOT = resolve(import.meta.dirname, "../..");
const scratch: string[] = [];
afterEach(() => {
	for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true });
});

function repo(config?: string): string {
	const root = mkdtempSync(join(tmpdir(), "f219-omp-config-"));
	scratch.push(root);
	if (config !== undefined) {
		mkdirSync(join(root, ".omp"));
		writeFileSync(join(root, CONFIG), config);
	}
	return root;
}

const noOmp = () => undefined;

describe("omp-config-check (F219)", () => {
	it("passes on this repository, by OMP's own answer when omp is installed", () => {
		expect(ompConfigFindings(ROOT)).toEqual([]);
	});

	it("fails when the project layer is missing", () => {
		expect(ompConfigFindings(repo(), noOmp)).toEqual([
			".omp/config.yml is missing; it must set worktree.clone: false",
		]);
	});

	it.each([
		["clone left on", "worktree:\n  clone: true\n"],
		["clone commented out", "worktree:\n  # clone: false\n"],
		["clone under another key", "other:\n  clone: false\nworktree:\n  mode: x\n"],
		// R260-001: nested below worktree is not worktree.clone (OMP answers true).
		["clone nested below worktree", "worktree:\n  unrelated:\n    clone: false\n"],
		["clone false then overridden", "worktree:\n  clone: false\n  clone: true\n"],
		["an inline mapping with clone on", "worktree: {clone: true}\n"],
		[
			"an inline mapping overridden by a block",
			"worktree: {clone: false}\nworktree:\n  clone: true\n",
		],
	])("fails with %s", (_, config) => {
		expect(ompConfigFindings(repo(config), noOmp)).toEqual([
			".omp/config.yml does not set worktree:\n  clone: false",
		]);
	});

	it("fails when OMP itself answers true, whatever the file says", () => {
		const findings = ompConfigFindings(repo("worktree:\n  clone: false\n"), () => true);
		expect(findings).toHaveLength(1);
		expect(findings[0]).toContain("answers true");
	});

	it("reads the committed block with its comments", () => {
		expect(setsCloneFalse("# why\nworktree:\n  clone: false # F219\n")).toBe(true);
	});

	// R260-002: a valid inline mapping is the same YAML value.
	it.each([
		"worktree: {clone: false}\n",
		"worktree: { mode: x, clone: false }\n",
		"worktree:\n  clone: true\nworktree: {clone: false} # last wins\n",
	])("reads an inline mapping: %j", (config) => {
		expect(setsCloneFalse(config)).toBe(true);
		expect(ompConfigFindings(repo(config), noOmp)).toEqual([]);
	});
});
