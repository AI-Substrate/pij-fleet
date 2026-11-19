import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// F217: vitest puts harness/test-tripwire first on every test's PATH, so a test
// that starts a cargo build fails instead of building pij-rs into a fresh
// per-worktree target.

const scratch: string[] = [];
afterEach(() => {
	for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true });
});

function cargo(args: string[]) {
	const cwd = mkdtempSync(join(tmpdir(), "f217-tripwire-"));
	scratch.push(cwd);
	const result = spawnSync("cargo", args, { cwd, encoding: "utf8" });
	return { ...result, cwd };
}

describe("the cargo tripwire on a test's PATH", () => {
	it.each([
		[["build", "--locked", "-p", "pij-cli", "--bin", "pij-rs"]],
		[["--color", "never", "test", "--workspace"]],
		[["+stable", "clippy"]],
	])("refuses a nested build: cargo %j", (args) => {
		const result = cargo(args);
		expect(result.status).toBe(101);
		expect(result.stderr).toContain("cargo tripwire (F217)");
		expect(readdirSync(result.cwd)).toEqual([]);
	});

	it("passes a command that builds nothing through to the real cargo", () => {
		const result = cargo(["--version"]);
		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toMatch(/^cargo \d+\.\d+/);
	});
});
