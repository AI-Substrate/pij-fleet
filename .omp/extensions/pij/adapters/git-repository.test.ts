import { execFileSync } from "node:child_process";
import { mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { removeTemporaryTree } from "../../../../harness/test-utils.js";

import { GitRepositoryAdapter } from "./git-repository.js";

const roots: string[] = [];

function tempRoot(): string {
	const root = mkdtempSync(join(tmpdir(), "pij-git-repository-"));
	roots.push(root);
	return root;
}

function git(args: readonly string[]): string {
	return execFileSync("git", args, {
		encoding: "utf8",
		stdio: ["ignore", "pipe", "pipe"],
	});
}

function initRepository(path: string): void {
	git(["init", "--quiet", path]);
	git(["-C", path, "config", "user.email", "pij@example.test"]);
	git(["-C", path, "config", "user.name", "pij test"]);
	git(["-C", path, "commit", "--quiet", "--allow-empty", "-m", "initial"]);
}

afterEach(() => {
	for (const root of roots.splice(0)) {
		removeTemporaryTree(root);
	}
});

describe("GitRepositoryAdapter", () => {
	it("resolves one canonical common directory for a main checkout and linked worktree", () => {
		const root = tempRoot();
		const main = join(root, "main");
		const linked = join(root, "linked");
		initRepository(main);
		git(["-C", main, "worktree", "add", "--quiet", "-b", "linked-branch", linked]);

		const repositories = new GitRepositoryAdapter();
		const mainKey = repositories.gitCommonDir(main);
		const linkedKey = repositories.gitCommonDir(linked);

		expect(mainKey).toBe(realpathSync(resolve(main, ".git")));
		expect(linkedKey).toBe(mainKey);
	});

	it("distinguishes unrelated repositories and returns null outside git", () => {
		const root = tempRoot();
		const first = join(root, "first");
		const second = join(root, "second");
		initRepository(first);
		initRepository(second);
		const repositories = new GitRepositoryAdapter();

		expect(repositories.gitCommonDir(first)).not.toBe(repositories.gitCommonDir(second));
		expect(repositories.gitCommonDir(root)).toBeNull();
		expect(repositories.gitCommonDir(join(root, "missing"))).toBeNull();
	});

	it("invokes git through an injected argv-only runner", () => {
		const calls: Array<{ command: string; args: readonly string[] }> = [];
		const repositories = new GitRepositoryAdapter((command, args) => {
			calls.push({ command, args });
			return "/repo/.git\n";
		});

		expect(repositories.gitCommonDir("/repo/worktree")).toBe("/repo/.git");
		expect(calls).toEqual([
			{
				command: "git",
				args: ["-C", "/repo/worktree", "rev-parse", "--path-format=absolute", "--git-common-dir"],
			},
		]);
	});
});
