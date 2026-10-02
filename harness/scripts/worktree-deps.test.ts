import { execFileSync, spawnSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readlinkSync,
	realpathSync,
	rmSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { removeTemporaryTree } from "../test-utils.js";

const script = join(import.meta.dirname, "worktree-deps.mjs");
let root: string;
let main: string;
let worktree: string;
let source: string;
let target: string;

function file(path: string, content = "fixture"): void {
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, content);
}

function run(args: string[] = [], cwd = worktree) {
	return spawnSync(process.execPath, [script, ...args], { cwd, encoding: "utf8" });
}

beforeEach(() => {
	root = mkdtempSync(join(tmpdir(), "pij-worktree-deps-"));
	main = join(root, 'main with "quotes"');
	worktree = join(root, "linked worktree");
	const git = (...args: string[]) => execFileSync("git", args, { encoding: "utf8" });
	git("init", "--quiet", main);
	git(
		"-C",
		main,
		"-c",
		"user.name=Fixture",
		"-c",
		"user.email=fixture@example.test",
		"-c",
		"commit.gpgsign=false",
		"commit",
		"--quiet",
		"--allow-empty",
		"-m",
		"fixture",
	);
	git("-C", main, "worktree", "add", "--quiet", "--detach", worktree);
	source = join(main, "node_modules");
	target = join(worktree, "node_modules");
	file(join(source, "unscoped", "index.js"), "export default 42;\n");
	file(join(source, "@org", "pkg", "index.js"));
	file(join(source, "@types", "node", "index.d.ts"));
	file(join(source, ".package-lock.json"));
	for (const name of ["vitest", "tsc"]) {
		const executable = join(source, name, "cli.js");
		file(executable, '#!/usr/bin/env node\nconsole.log("fixture binary");\n');
		chmodSync(executable, 0o755);
		mkdirSync(join(source, ".bin"), { recursive: true });
		symlinkSync(`../${name}/cli.js`, join(source, ".bin", name));
	}
});

afterEach(() => removeTemporaryTree(root));

describe("worktree-deps CLI", () => {
	it("links unscoped/scoped packages and executable bins without copying or nesting node_modules", () => {
		file(join(source, "node_modules", "accidental", "index.js"));
		const result = run();
		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toContain("7 links (7 created, 0 unchanged)");
		expect(result.stdout).toContain("READY (.bin/vitest, .bin/tsc, @types/node resolve)");
		for (const container of [
			target,
			join(target, "@org"),
			join(target, "@types"),
			join(target, ".bin"),
		]) {
			expect(lstatSync(container).isSymbolicLink()).toBe(false);
			expect(lstatSync(container).isDirectory()).toBe(true);
		}
		for (const name of ["unscoped", "@org/pkg", "@types/node", ".bin/vitest", ".bin/tsc"]) {
			expect(lstatSync(join(target, name)).isSymbolicLink()).toBe(true);
			expect(realpathSync(join(target, name))).toBe(realpathSync(join(source, name)));
		}
		expect(existsSync(join(target, "node_modules"))).toBe(false);
		expect(existsSync(join(target, ".package-lock.json"))).toBe(false);
		expect(execFileSync(join(target, ".bin/vitest"), { encoding: "utf8" })).toBe(
			"fixture binary\n",
		);
		file(join(source, "unscoped", "index.js"), "changed at source");
		expect(readFileSync(join(target, "unscoped", "index.js"), "utf8")).toBe("changed at source");
	});

	it("is idempotent and accepts an explicit checkout from a worktree subdirectory", () => {
		mkdirSync(join(worktree, "src"));
		expect(run([main], join(worktree, "src")).status).toBe(0);
		const before = lstatSync(join(target, "unscoped")).ino;
		const second = run([main]);
		expect(second.status, second.stderr).toBe(0);
		expect(second.stdout).toContain("7 links (0 created, 7 unchanged)");
		expect(lstatSync(join(target, "unscoped")).ino).toBe(before);
	});

	it.each([
		"unscoped",
		"@org/pkg",
		".bin/vitest",
		"orphan",
		"@other/orphan",
	])("refuses real package/binary %s before writing any links", (name) => {
		file(join(target, name, "keep"), "owned data");
		const result = run();
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("Refusing real package or binary");
		expect(readFileSync(join(target, name, "keep"), "utf8")).toBe("owned data");
		expect(existsSync(join(target, "tsc"))).toBe(false);
	});

	it.each([
		"",
		"@org",
		".bin",
	])("refuses symlink container %s rather than writing through to main", (name) => {
		if (name) mkdirSync(target);
		symlinkSync(join(source, name), join(target, name));
		const result = run();
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("symlink container");
		expect(lstatSync(join(source, "unscoped")).isDirectory()).toBe(true);
	});

	it("repairs stale package links but never replaces real directories", () => {
		expect(run().status).toBe(0);
		unlinkSync(join(target, "unscoped"));
		symlinkSync(join(root, "missing"), join(target, "unscoped"));
		const result = run();
		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toContain("1 created, 6 unchanged");
		expect(readlinkSync(join(target, "unscoped"))).toBe(join(realpathSync(source), "unscoped"));
	});

	it("checks read-only, prescribes the exact repair, and detects broken required links", () => {
		const missing = run(["--check"]);
		expect(missing.status).toBe(1);
		expect(missing.stderr).toContain("NOT-READY");
		expect(missing.stderr).toContain("Run `just worktree-deps`");
		expect(missing.stderr).toContain("Rust-only work can continue");
		expect(existsSync(target)).toBe(false);
		expect(run().status).toBe(0);
		expect(run(["--check"]).stdout).toContain("READY (");
		rmSync(join(source, "tsc"), { recursive: true });
		expect(run(["--check"]).status).toBe(1);
	});

	it("refuses an unusable source before creating the destination", () => {
		rmSync(join(source, "@types", "node"), { recursive: true });
		const result = run();
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("@types/node");
		expect(existsSync(target)).toBe(false);
	});

	it("does not create cycles when source bins alias the target bin container", () => {
		mkdirSync(join(target, ".bin"), { recursive: true });
		for (const name of ["vitest", "tsc"]) {
			symlinkSync(join(source, name, "cli.js"), join(target, ".bin", name));
		}
		rmSync(join(source, ".bin"), { recursive: true });
		symlinkSync(join(target, ".bin"), join(source, ".bin"));
		const result = run();
		expect(result.status, result.stderr).toBe(0);
		expect(realpathSync(join(source, ".bin", "vitest"))).toBe(
			realpathSync(join(source, "vitest", "cli.js")),
		);
	});

	it("resolves source package aliases through existing target links before relinking", () => {
		const outside = join(root, "external-package");
		file(join(outside, "index.js"));
		mkdirSync(target);
		symlinkSync(outside, join(target, "unscoped"));
		rmSync(join(source, "unscoped"), { recursive: true });
		symlinkSync(join(target, "unscoped"), join(source, "unscoped"));
		const result = run();
		expect(result.status, result.stderr).toBe(0);
		expect(realpathSync(join(source, "unscoped"))).toBe(realpathSync(outside));
		expect(realpathSync(join(target, "unscoped"))).toBe(realpathSync(outside));
	});

	it("refuses source packages that resolve inside the target dependency directory", () => {
		file(join(target, ".cache", "unscoped", "index.js"));
		rmSync(join(source, "unscoped"), { recursive: true });
		symlinkSync(join(target, ".cache", "unscoped"), join(source, "unscoped"));
		const result = run();
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("Source package must not be inside");
		expect(existsSync(join(target, "tsc"))).toBe(false);
	});

	it("preserves trailing whitespace in the checkout path instead of mutating a sibling", () => {
		const spaced = join(root, "trailing ");
		execFileSync("git", ["-C", main, "worktree", "move", worktree, spaced]);
		worktree = spaced;
		target = join(worktree, "node_modules");
		mkdirSync(spaced.trimEnd());
		const result = run();
		expect(result.status, result.stderr).toBe(0);
		expect(existsSync(join(target, ".bin", "vitest"))).toBe(true);
		expect(existsSync(join(spaced.trimEnd(), "node_modules"))).toBe(false);
	});

	it("never modifies the canonical checkout or links a worktree to itself", () => {
		expect(run([], main).stderr).toContain("Refusing to modify the canonical checkout");
		expect(run(["--check"], main).status).toBe(0);
		expect(run().status).toBe(0);
		expect(run([worktree]).stderr).toContain("Source and target must be different");
		expect(lstatSync(join(source, "unscoped")).isSymbolicLink()).toBe(false);
	});
});
