import { spawnSync } from "node:child_process";
import {
	chmodSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	realpathSync,
	renameSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const RESOLVER = resolve(import.meta.dirname, "government-root.sh");
const REROOT = resolve(import.meta.dirname, "reroot-governance.sh");
const SOURCE = "refs/heads/prime-governance";
const TAG = "refs/tags/prime-governance-pre-orphan-2026-09-07";
const CANDIDATE = "refs/heads/prime-governance-orphan-2026-09-07";
const roots: string[] = [];

afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function executable(path: string, body: string): void {
	writeFileSync(path, `#!/bin/sh\nset -eu\n${body}\n`);
	chmodSync(path, 0o755);
}

// Byte snapshots include Git's objects, refs, reflogs and indexes: dry-run must not
// merely leave the branch tip alone while refreshing an index or creating objects.
function snapshot(root: string): Record<string, string> {
	const files: Record<string, string> = {};
	function visit(directory: string, prefix: string): void {
		for (const name of readdirSync(directory).sort()) {
			const path = join(directory, name);
			const key = `${prefix}${name}`;
			const stat = lstatSync(path);
			if (stat.isSymbolicLink()) files[key] = `link:${readlinkSync(path)}`;
			else if (stat.isDirectory()) {
				files[`${key}/`] = "directory";
				visit(path, `${key}/`);
			} else files[key] = readFileSync(path).toString("base64");
		}
	}
	visit(root, "");
	return files;
}

function fixture(options: { worktree?: boolean; branch?: boolean; unusualPath?: boolean } = {}) {
	const root = realpathSync(mkdtempSync(join(tmpdir(), "pij-governance-")));
	roots.push(root);
	const repo = join(root, "product checkout");
	const standing = join(
		root,
		options.unusualPath ? 'government "quoted"\tline\nnext' : "standing government",
	);
	const bin = join(root, "bin");
	mkdirSync(repo);
	mkdirSync(bin);
	const env: NodeJS.ProcessEnv = Object.fromEntries(
		Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
	);
	Object.assign(env, {
		PATH: `${bin}:${process.env.PATH}`,
		GIT_CONFIG_NOSYSTEM: "1",
		GIT_CONFIG_GLOBAL: "/dev/null",
		GIT_OPTIONAL_LOCKS: "0",
		GIT_AUTHOR_NAME: "Fixture",
		GIT_AUTHOR_EMAIL: "fixture@example.invalid",
		GIT_COMMITTER_NAME: "Fixture",
		GIT_COMMITTER_EMAIL: "fixture@example.invalid",
		GIT_AUTHOR_DATE: "2026-09-07T00:00:00Z",
		GIT_COMMITTER_DATE: "2026-09-07T00:00:00Z",
		TRAILERS_LOG: join(root, "trailer-calls"),
		TRAILERS_HOOK: "",
		LC_ALL: "C",
	});
	const gitResult = (cwd: string, ...args: string[]) =>
		spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8", env });
	const gitAt = (cwd: string, ...args: string[]) => {
		const result = gitResult(cwd, ...args);
		if (result.status !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
		return result.stdout.trim();
	};
	const git = (...args: string[]) => gitAt(repo, ...args);
	git("init", "--initial-branch=main");
	git("config", "core.hooksPath", "/dev/null");
	mkdirSync(join(repo, ".harness", "government"), { recursive: true });
	writeFileSync(join(repo, ".harness", "government", "spine.md"), "source government\n");
	writeFileSync(join(repo, "product.txt"), "product stays unchanged\n");
	git("add", ".");
	git("commit", "-m", "product root");
	if (options.branch !== false) git("branch", "prime-governance");
	if (options.worktree !== false) {
		git("worktree", "add", standing, "prime-governance");
		writeFileSync(
			join(standing, ".harness", "government", "spine.md"),
			"current government tree\n",
		);
		gitAt(standing, "add", ".");
		gitAt(standing, "commit", "-m", "governance history");
	}
	executable(
		join(bin, "pij-rs"),
		'[ "$#" -eq 1 ] && [ "$1" = commit-trailers ]\nprintf "called\\n" >> "$TRAILERS_LOG"\nif [ -n "$TRAILERS_HOOK" ]; then "$TRAILERS_HOOK"; fi\nprintf "Pij-Prime: pij-fixture-prime\\n"',
	);
	return {
		root,
		repo,
		standing,
		bin,
		env,
		git,
		gitAt,
		gitResult,
		resolve: (cwd = repo, ...args: string[]) =>
			spawnSync(RESOLVER, args, { cwd, encoding: "utf8", env }),
		reroot: (...args: string[]) => spawnSync(REROOT, args, { cwd: repo, encoding: "utf8", env }),
		refExists: (ref: string) =>
			gitResult(repo, "show-ref", "--verify", "--quiet", ref).status === 0,
	};
}

function handoff(output: string): string {
	const match = output.match(/# BEGIN PRIME HANDOFF\n([\s\S]*?)# END PRIME HANDOFF/);
	if (!match) throw new Error("No prime handoff block in command output");
	return match[1];
}

function runHandoff(f: ReturnType<typeof fixture>, output: string) {
	return spawnSync("bash", ["-c", handoff(output)], { cwd: f.repo, encoding: "utf8", env: f.env });
}

describe("government-root", () => {
	it("resolves the exact registered worktree from cwd or explicit repo, preserving unusual paths", () => {
		const f = fixture({ unusualPath: true });
		for (const result of [f.resolve(), f.resolve(f.root, f.repo), f.resolve(f.standing)]) {
			expect(result.status, result.stderr).toBe(0);
			expect(result.stdout).toBe(`${f.standing}/.harness/government\n`);
			expect(result.stderr).toBe("");
		}
	});

	it("resolves a fresh orphan created by the generic bootstrap recipe", () => {
		const f = fixture({ worktree: false, branch: false });
		f.git("worktree", "add", "--orphan", "-b", "prime-governance", f.standing);
		const government = join(f.standing, ".harness", "government");
		mkdirSync(government, { recursive: true });
		writeFileSync(join(government, "spine.md"), "fresh government\n");
		f.gitAt(f.standing, "add", ".harness/government/spine.md");
		f.gitAt(f.standing, "commit", "-m", "governance root");
		const result = f.resolve();
		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toBe(`${government}\n`);
		expect(f.git("rev-list", "--count", SOURCE)).toBe("1");
		expect(f.git("ls-tree", "-r", "--name-only", SOURCE)).toBe(".harness/government/spine.md");
	});

	it.each([
		true,
		false,
	])("refuses a local government tree without a standing worktree (branch exists: %s)", (branch) => {
		const f = fixture({ worktree: false, branch });
		const result = f.resolve();
		expect(result.status).toBe(1);
		expect(result.stdout).toBe("");
		expect(result.stderr).toContain("Bootstrap: /pij prime");
		if (branch)
			expect(result.stderr).toContain("worktree add <standing-worktree> prime-governance");
	});

	it("refuses an unavailable registered worktree without pruning it", () => {
		const f = fixture();
		renameSync(f.standing, `${f.standing}-unavailable`);
		const before = snapshot(f.root);
		const result = f.resolve();
		expect(result.status).toBe(1);
		expect(result.stdout).toBe("");
		expect(result.stderr).toContain("unavailable");
		expect(result.stderr).toContain("Bootstrap: /pij prime");
		expect(snapshot(f.root)).toEqual(before);
	});

	it("refuses an absent government directory instead of returning the main copy", () => {
		const f = fixture();
		rmSync(join(f.standing, ".harness", "government"), { recursive: true });
		const result = f.resolve();
		expect(result.status).toBe(1);
		expect(result.stdout).toBe("");
		expect(result.stderr).toContain("government directory is unavailable");
	});
});

describe("reroot-governance", () => {
	it("dry-runs without changing any files or refs or contacting attribution", () => {
		const f = fixture();
		const source = f.git("rev-parse", SOURCE);
		const tree = f.git("rev-parse", `${SOURCE}^{tree}`);
		const before = snapshot(f.root);
		const result = f.reroot("--dry-run");
		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toContain(`Source tip: ${source}`);
		expect(result.stdout).toContain(`Source tree: ${tree}`);
		expect(result.stdout).toContain(TAG.slice("refs/tags/".length));
		expect(result.stdout).toContain(CANDIDATE);
		expect(result.stdout).toContain("pij-rs commit-trailers");
		expect(snapshot(f.root)).toEqual(before);
		const premature = runHandoff(f, result.stdout);
		expect(premature.status).not.toBe(0);
		expect(snapshot(f.root)).toEqual(before);
	});

	it("prepares a single attributed orphan with the exact tree without touching the standing checkout or source", () => {
		const f = fixture();
		const source = f.git("rev-parse", SOURCE);
		const tree = f.git("rev-parse", `${SOURCE}^{tree}`);
		const main = f.git("rev-parse", "main");
		const worktreeGitDir = f.gitAt(f.standing, "rev-parse", "--absolute-git-dir");
		const beforeWorktree = snapshot(f.standing);
		const beforeWorktreeGit = snapshot(worktreeGitDir);
		const result = f.reroot("--prepare", f.repo);
		expect(result.status, result.stderr).toBe(0);
		const candidate = f.git("rev-parse", CANDIDATE);
		expect(f.git("rev-parse", `${CANDIDATE}^{tree}`)).toBe(tree);
		expect(f.git("rev-list", "--parents", "-n", "1", CANDIDATE)).toBe(candidate);
		expect(f.git("rev-list", "--count", CANDIDATE)).toBe("1");
		expect(f.git("rev-parse", TAG)).toBe(source);
		expect(f.git("rev-parse", SOURCE)).toBe(source);
		expect(f.git("rev-parse", "main")).toBe(main);
		expect(f.gitAt(f.standing, "symbolic-ref", "HEAD")).toBe(SOURCE);
		expect(snapshot(f.standing)).toEqual(beforeWorktree);
		expect(snapshot(worktreeGitDir)).toEqual(beforeWorktreeGit);
		expect(f.git("show", "-s", "--format=%B", candidate)).toBe(
			"governance: orphan re-root, history at tag prime-governance-pre-orphan-2026-09-07\n\nPij-Prime: pij-fixture-prime",
		);
	});

	it.each([TAG, CANDIDATE])("refuses an existing %s without clobbering any ref", (ref) => {
		const f = fixture();
		f.git("update-ref", ref, "main");
		const before = snapshot(f.root);
		const result = f.reroot("--prepare");
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("ref already exists");
		expect(snapshot(f.root)).toEqual(before);
	});

	it.each([
		"tracked",
		"untracked",
		"staged",
	])("refuses a %s dirty standing worktree before preparation", (kind) => {
		const f = fixture();
		writeFileSync(
			join(f.standing, kind === "untracked" ? "new.txt" : "product.txt"),
			"preserve my work\n",
		);
		if (kind === "staged") f.gitAt(f.standing, "add", ".");
		const before = snapshot(f.root);
		const result = f.reroot("--prepare");
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("dirty");
		expect(snapshot(f.root)).toEqual(before);
	});

	it("refuses an in-progress operation even if the index and worktree are clean", () => {
		const f = fixture();
		writeFileSync(
			f.gitAt(f.standing, "rev-parse", "--path-format=absolute", "--git-path", "MERGE_HEAD"),
			`${f.git("rev-parse", "main")}\n`,
		);
		const before = snapshot(f.root);
		const result = f.reroot("--prepare");
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("operation in progress");
		expect(snapshot(f.root)).toEqual(before);
	});

	it("refuses a source that advances during attribution without creating backup or candidate refs", () => {
		const f = fixture();
		const hook = join(f.bin, "advance-source");
		f.env.TRAILERS_HOOK = hook;
		f.env.STANDING = f.standing;
		executable(hook, 'git -C "$STANDING" commit --allow-empty -m "concurrent governance commit"');
		const before = f.git("rev-parse", SOURCE);
		const result = f.reroot("--prepare");
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("source tip changed");
		expect(f.git("rev-parse", SOURCE)).not.toBe(before);
		expect(f.refExists(TAG)).toBe(false);
		expect(f.refExists(CANDIDATE)).toBe(false);
	});

	it("fails closed when attribution is unavailable", () => {
		const f = fixture();
		executable(join(f.bin, "pij-rs"), "exit 1");
		const before = snapshot(f.root);
		const result = f.reroot("--prepare");
		expect(result.status).toBe(1);
		expect(result.stderr).toContain("commit-trailers failed");
		expect(snapshot(f.root)).toEqual(before);
	});

	it("hands off safely from a checked-out branch and preserves the standing files", () => {
		const f = fixture({ unusualPath: true });
		const hooks = join(f.root, "hooks");
		mkdirSync(hooks);
		f.env.CHECKOUT_HOOK_LOG = join(f.root, "checkout-hooks");
		executable(join(hooks, "post-checkout"), 'printf "checkout\\n" >> "$CHECKOUT_HOOK_LOG"');
		f.git("config", "core.hooksPath", hooks);
		const before = snapshot(f.standing);
		const result = f.reroot("--prepare");
		expect(result.status, result.stderr).toBe(0);
		const transition = runHandoff(f, result.stdout);
		expect(transition.status, transition.stderr).toBe(0);
		expect(f.gitAt(f.standing, "symbolic-ref", "HEAD")).toBe(SOURCE);
		expect(f.git("rev-parse", SOURCE)).toBe(f.git("rev-parse", CANDIDATE));
		expect(f.git("rev-list", "--count", SOURCE)).toBe("1");
		expect(snapshot(f.standing)).toEqual(before);
		expect(readFileSync(f.env.CHECKOUT_HOOK_LOG, "utf8")).toBe("checkout\ncheckout\n");
	});

	it.each([
		"dirty",
		"advanced",
		"detached",
	])("refuses a %s worktree at prime handoff without resetting it", (state) => {
		const f = fixture();
		const result = f.reroot("--prepare");
		expect(result.status, result.stderr).toBe(0);
		if (state === "dirty") writeFileSync(join(f.standing, "product.txt"), "preserve this\n");
		if (state === "advanced")
			f.gitAt(f.standing, "commit", "--allow-empty", "-m", "new governance work");
		if (state === "detached") f.gitAt(f.standing, "switch", "--detach", "main");
		const before = snapshot(f.root);
		const transition = runHandoff(f, result.stdout);
		expect(transition.status).not.toBe(0);
		expect(snapshot(f.root)).toEqual(before);
	});
});
