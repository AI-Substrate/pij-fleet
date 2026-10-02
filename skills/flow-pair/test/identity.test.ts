// skills/flow-pair/test/identity.test.ts
// P8: tests target the lib directly, not CLI wiring.
// Real tmp git fixtures per T001 task notes (no mocks).

import { execSync } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { removeTemporaryTree } from "../../../harness/test-utils.js";
import { deriveRepoId, type GitDeps } from "../lib/identity.js";

describe("deriveRepoId", () => {
	let tmpDir: string;

	beforeEach(async () => {
		tmpDir = await mkdtemp(join(tmpdir(), "fp-test-"));
	});

	afterEach(() => {
		removeTemporaryTree(tmpDir);
	});

	it("derives host-owner-repo from HTTPS remote", () => {
		execSync("git init", { cwd: tmpDir });
		execSync("git remote add origin https://github.com/foo/bar.git", {
			cwd: tmpDir,
		});
		const result = deriveRepoId(tmpDir);
		expect(result.ok).toBe(true);
		expect(result.repoId).toBe("github.com-foo-bar");
	});

	it("derives host-owner-repo from SSH remote", () => {
		execSync("git init", { cwd: tmpDir });
		execSync("git remote add origin git@github.com:foo/bar.git", {
			cwd: tmpDir,
		});
		const result = deriveRepoId(tmpDir);
		expect(result.ok).toBe(true);
		expect(result.repoId).toBe("github.com-foo-bar");
	});

	it("falls back to basename-hash when git has no remote", () => {
		execSync("git init", { cwd: tmpDir });
		// No remote added
		const result = deriveRepoId(tmpDir);
		expect(result.ok).toBe(true);
		expect(result.repoId).toMatch(/^.+-[0-9a-f]{8}$/);
		expect(result.repoId.startsWith(basename(tmpDir))).toBe(true);
	});

	it("falls back to basename-hash when no .git dir at all", () => {
		// Plain tmpdir, no git init
		const result = deriveRepoId(tmpDir);
		expect(result.ok).toBe(true);
		expect(result.repoId).toMatch(/^.+-[0-9a-f]{8}$/);
	});

	it("is stable across repeated calls", () => {
		execSync("git init", { cwd: tmpDir });
		execSync("git remote add origin https://github.com/myorg/myrepo.git", {
			cwd: tmpDir,
		});
		const r1 = deriveRepoId(tmpDir);
		const r2 = deriveRepoId(tmpDir);
		expect(r1.repoId).toBe(r2.repoId);
	});

	it("accepts injected fake GitDeps (P3)", () => {
		const fakeDeps: GitDeps = {
			getRemoteOriginUrl: (_path: string) => "https://github.com/injected/repo.git",
		};
		const result = deriveRepoId("/some/path", fakeDeps);
		expect(result.ok).toBe(true);
		expect(result.repoId).toBe("github.com-injected-repo");
	});
});
