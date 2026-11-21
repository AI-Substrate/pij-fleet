import { execFileSync } from "node:child_process";
import {
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	contractCommit,
	passDir,
	readLocalPass,
	writeLocalPass,
} from "./copilot-sdk-local-pass.mjs";

const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});
const fresh = () => {
	const root = mkdtempSync(join(tmpdir(), "pij-copilot-local-pass-"));
	roots.push(root);
	return root;
};
const PASSED_AT = new Date("2026-11-21T12:00:00.000Z");
const SHA = "0123456789abcdef0123456789abcdef01234567";

describe("a local Copilot contract pass (F250)", () => {
	it("lives under pij-rs's state dir", () => {
		expect(passDir({ PIJ_RS_STATE_DIR: "/s", HOME: "/h" })).toBe("/s/copilot-sdk-pass");
		expect(passDir({ HOME: "/h" })).toBe("/h/.pij-rs/copilot-sdk-pass");
	});

	it("reads back exactly the version it was written for, and no other", async () => {
		const dir = fresh();
		await writeLocalPass(dir, {
			version: "1.0.115-2",
			passedAt: PASSED_AT,
			checks: 7,
			contractCommit: SHA,
		});
		expect(await readLocalPass(dir, "1.0.115-2", 7)).toEqual({
			version: "1.0.115-2",
			passed_at: "2026-11-21T12:00:00.000Z",
			checks: 7,
			contract_commit: SHA,
		});
		expect(await readLocalPass(dir, "1.0.115-3", 7)).toBeUndefined();
	});

	it.each([
		["corrupt JSON", '{"version": "1.0.115-3", "passed_at":'],
		["empty", ""],
		["another version's record under this name", { version: "1.0.115-2" }],
		["no pass time", { passed_at: undefined }],
		["a pass time that is not a time", { passed_at: "yesterday" }],
		["no check count", { checks: undefined }],
		["a different check count", { checks: 6 }],
		["a commit that is not a sha", { contract_commit: "main" }],
		["not an object", "[]"],
	])("treats %s as absent", async (_case, content) => {
		const dir = fresh();
		const whole = {
			version: "1.0.115-3",
			passed_at: "2026-11-21T12:00:00.000Z",
			checks: 7,
			contract_commit: null,
		};
		writeFileSync(
			join(dir, "1.0.115-3.json"),
			typeof content === "string" ? content : JSON.stringify({ ...whole, ...content }),
		);
		expect(await readLocalPass(dir, "1.0.115-3", 7)).toBeUndefined();
	});

	it("never follows a symlink at the record's path", async () => {
		const dir = fresh();
		const elsewhere = fresh();
		await writeLocalPass(elsewhere, { version: "1.0.115-3", passedAt: PASSED_AT, checks: 7 });
		symlinkSync(join(elsewhere, "1.0.115-3.json"), join(dir, "1.0.115-3.json"));
		expect(await readLocalPass(elsewhere, "1.0.115-3", 7)).toBeDefined();
		expect(await readLocalPass(dir, "1.0.115-3", 7)).toBeUndefined();
	});

	it("refuses a version that is a path, writing nothing", async () => {
		const dir = fresh();
		await expect(
			writeLocalPass(join(dir, "pass"), { version: "../x", passedAt: PASSED_AT, checks: 7 }),
		).rejects.toThrow("not a Copilot version");
		expect(readdirSync(dir)).toEqual([]);
		expect(await readLocalPass(dir, "../x", 7)).toBeUndefined();
	});

	it("replaces a record whole and leaves no staging file", async () => {
		const dir = fresh();
		await writeLocalPass(dir, { version: "1.0.115-3", passedAt: PASSED_AT, checks: 6 });
		await writeLocalPass(dir, { version: "1.0.115-3", passedAt: PASSED_AT, checks: 7 });
		expect(readdirSync(dir)).toEqual(["1.0.115-3.json"]);
		expect(JSON.parse(readFileSync(join(dir, "1.0.115-3.json"), "utf8")).checks).toBe(7);
	});
});

describe("the contract's commit, read without launching git (F250)", () => {
	const git = (cwd: string, ...args: string[]) =>
		execFileSync("/usr/bin/git", args, {
			cwd,
			encoding: "utf8",
			env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" },
		}).trim();

	it("names HEAD in a checkout, a linked worktree, and after the refs are packed", async () => {
		const root = fresh();
		const main = join(root, "main");
		mkdirSync(main);
		git(main, "init", "-q");
		git(
			main,
			"-c",
			"user.name=t",
			"-c",
			"user.email=t@t",
			"commit",
			"-q",
			"--allow-empty",
			"-m",
			"a",
		);
		const linked = join(root, "linked");
		git(main, "worktree", "add", "-q", "-b", "lane", linked);
		git(
			linked,
			"-c",
			"user.name=t",
			"-c",
			"user.email=t@t",
			"commit",
			"-q",
			"--allow-empty",
			"-m",
			"b",
		);
		expect(await contractCommit(main)).toBe(git(main, "rev-parse", "HEAD"));
		expect(await contractCommit(linked)).toBe(git(linked, "rev-parse", "HEAD"));
		git(main, "pack-refs", "--all");
		expect(await contractCommit(main)).toBe(git(main, "rev-parse", "HEAD"));
		expect(await contractCommit(linked)).toBe(git(linked, "rev-parse", "HEAD"));
		git(main, "checkout", "-q", "--detach");
		expect(await contractCommit(main)).toBe(git(main, "rev-parse", "HEAD"));
		expect(await contractCommit(join(root, "absent"))).toBeUndefined();
	});
});
