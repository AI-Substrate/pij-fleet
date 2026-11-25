import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { gitEnv } from "../driver/git-env.js";

// cargo's target runner (incident #354): every test binary starts through it.
const RUNNER = join(import.meta.dirname, "cargo-test-runner.sh");

const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/** A stand-in test binary under deps/ that records the environment it got. */
function fakeTestBinary(): { binary: string; record: string } {
	const root = mkdtempSync(join(tmpdir(), "pij-cargo-runner-"));
	roots.push(root);
	mkdirSync(join(root, "deps"));
	const record = join(root, "env.txt");
	const binary = join(root, "deps", "fake-test");
	writeFileSync(binary, `#!/bin/sh\nenv > '${record}'\n`);
	chmodSync(binary, 0o755);
	return { binary, record };
}

describe("cargo-test-runner.sh", () => {
	it("refuses a test binary while GIT_DIR or GIT_WORK_TREE is set, and runs nothing", () => {
		const { binary, record } = fakeTestBinary();
		const run = spawnSync(RUNNER, [binary], {
			env: gitEnv({ GIT_WORK_TREE: "/nonexistent/pij-runner-probe" }),
			encoding: "utf8",
		});
		expect(run.status).toBe(2);
		expect(run.stderr).toContain("this environment sets GIT_WORK_TREE");
		expect(run.stderr).toContain("fix: unset GIT_DIR GIT_WORK_TREE");
		expect(() => readFileSync(record)).toThrow();
	});

	it("starts a test binary without the inherited git location and config variables", () => {
		const { binary, record } = fakeTestBinary();
		const run = spawnSync(RUNNER, [binary], {
			env: gitEnv({
				GIT_INDEX_FILE: "/nonexistent/index",
				GIT_CONFIG_GLOBAL: "/nonexistent/config",
				GIT_CONFIG_COUNT: "1",
				GIT_CONFIG_KEY_0: "core.worktree",
				GIT_CONFIG_VALUE_0: "/nonexistent",
				PIJ_RUNNER_PROBE: "kept",
			}),
			encoding: "utf8",
		});
		expect(run.status).toBe(0);
		const seen = readFileSync(record, "utf8");
		expect(seen).not.toMatch(/^GIT_(INDEX_FILE|CONFIG)/m);
		expect(seen).toContain("PIJ_RUNNER_PROBE=kept");
	});

	it("passes anything outside deps/ (cargo run) through untouched", () => {
		const run = spawnSync(RUNNER, ["/usr/bin/env"], {
			env: gitEnv({ GIT_DIR: "/nonexistent/pij-runner-probe" }),
			encoding: "utf8",
		});
		expect(run.status).toBe(0);
		expect(run.stdout).toContain("GIT_DIR=/nonexistent/pij-runner-probe");
	});
});
