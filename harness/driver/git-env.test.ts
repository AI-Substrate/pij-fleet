import { execFileSync } from "node:child_process";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { gitEnv } from "./git-env.js";

// Incident #354: the decoy stands in for the real shared repository. The
// fixture's git children get GIT_DIR/GIT_WORK_TREE pointing at it, as they
// did when a seat exported them; gitEnv() must keep every write in the
// fixture's own directory.

const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function git(cwd: string, args: readonly string[], env: NodeJS.ProcessEnv): void {
	execFileSync("git", args, { cwd, env, stdio: ["ignore", "ignore", "pipe"] });
}

/** Every byte of the decoy's config, HEAD, refs, packed refs and worktrees. */
function snapshot(gitDir: string): Map<string, string> {
	const files = new Map<string, string>();
	const walk = (path: string): void => {
		if (!existsSync(path)) return;
		if (statSync(path).isDirectory()) {
			for (const entry of readdirSync(path)) walk(join(path, entry));
		} else {
			files.set(relative(gitDir, path), readFileSync(path).toString("base64"));
		}
	};
	for (const name of ["config", "HEAD", "packed-refs", "refs", "worktrees"])
		walk(join(gitDir, name));
	return files;
}

function changed(before: Map<string, string>, after: Map<string, string>): string[] {
	const names = new Set([...before.keys(), ...after.keys()]);
	return [...names].filter((name) => before.get(name) !== after.get(name)).sort();
}

describe("gitEnv (incident #354)", () => {
	it("keeps a fixture run under an inherited GIT_DIR/GIT_WORK_TREE out of the decoy", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-git-env-decoy-"));
		roots.push(root);
		const decoy = join(root, "decoy");
		const scratch = join(root, "scratch");
		mkdirSync(decoy);
		mkdirSync(scratch);
		const clean = gitEnv();
		git(decoy, ["init", "-q", "-b", "main"], clean);
		git(
			decoy,
			[
				"-c",
				"user.name=decoy",
				"-c",
				"user.email=decoy@example.invalid",
				"commit",
				"-q",
				"--allow-empty",
				"-m",
				"decoy",
			],
			clean,
		);
		const before = snapshot(join(decoy, ".git"));

		const polluted = { ...process.env, GIT_DIR: join(decoy, ".git"), GIT_WORK_TREE: decoy };
		const env = gitEnv({}, { base: polluted, ownIdentity: true });
		for (const args of [
			["init", "-q", "-b", "main"],
			["config", "user.name", "pij test"],
			["config", "user.email", "pij@example.invalid"],
			["commit", "-q", "--allow-empty", "-m", "fixture"],
			["branch", "dup"],
			["worktree", "add", "-q", "-b", "wt-branch", join(root, "scratch-linked")],
		]) {
			git(scratch, args, env);
		}

		expect(changed(before, snapshot(join(decoy, ".git")))).toEqual([]);
		expect(existsSync(join(scratch, ".git", "config"))).toBe(true);
	});
});
