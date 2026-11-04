import { type SpawnOptions, spawn, spawnSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	realpathSync,
	rmSync,
	statSync,
	utimesSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// Plan 198: `just shared-build`, driven with a fake cargo and a fake df in
// temporary directories. Never the real shared target.
const SCRIPT = resolve(import.meta.dirname, "shared-build.sh");
const GIB_KIB = 1_048_576;
const roots: string[] = [];

afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function executable(path: string, body: string): void {
	writeFileSync(path, `#!/bin/sh\n${body}\n`);
	chmodSync(path, 0o755);
}

interface Fixture {
	root: string;
	worktree: string;
	target: string;
	lock: string;
	log: string;
	free: string;
	env: NodeJS.ProcessEnv;
}

function fixture(): Fixture {
	const root = realpathSync(mkdtempSync(join(tmpdir(), "pij-shared-build-")));
	roots.push(root);
	const bin = join(root, "bin");
	const worktree = join(root, "worktree");
	mkdirSync(bin);
	for (const [crate, file] of [
		["alpha", "lib.rs"],
		["beta", "main.rs"],
	]) {
		mkdirSync(join(worktree, "crates", crate, "src"), { recursive: true });
		writeFileSync(join(worktree, "crates", crate, "src", file), "\n");
	}
	spawnSync("git", ["init", "-q", worktree]);
	const target = join(root, "target");
	const log = join(root, "cargo.log");
	const free = join(root, "free-kib");
	writeFileSync(free, String(40 * GIB_KIB));
	// Logs each run's start and end, its environment and the lock owner seen
	// while it runs; `drop` lowers the free space, `fail` exits non-zero.
	executable(
		join(bin, "cargo"),
		[
			'echo "start $1 target=$CARGO_TARGET_DIR incremental=$CARGO_INCREMENTAL" >> "$LOG"',
			'cat "$CARGO_TARGET_DIR.lock/owner" >> "$LOG"',
			'[ "$1" = drop ] && echo 1024 > "$FREE"',
			// An orphaned sleep must not hold the test's stderr pipe open.
			'[ -n "$NAP" ] && sleep "$NAP" </dev/null >/dev/null 2>&1',
			'echo "end $1" >> "$LOG"',
			'[ "$1" = fail ] && exit 7',
			"exit 0",
		].join("\n"),
	);
	const df = join(root, "df");
	executable(
		df,
		[
			'echo "Filesystem 1024-blocks Used Available Capacity Mounted on"',
			'echo "fake 100000000 0 $(cat "$FREE") 0% /"',
		].join("\n"),
	);
	return {
		root,
		worktree,
		target,
		lock: `${target}.lock`,
		log,
		free,
		env: {
			...process.env,
			PATH: `${bin}:${process.env.PATH}`,
			PIJ_SHARED_TARGET: target,
			PIJ_SHARED_BUILD_DF: df,
			PIJ_SHARED_BUILD_POLL_SECONDS: "1",
			PIJ_DISK_FLOOR_GB: "25",
			PIJ_ID: "pij-test-seat",
			LOG: log,
			FREE: free,
			NAP: "",
		},
	};
}

interface Run {
	code: number | null;
	stderr: string;
}

function start(f: Fixture, args: string[], env: NodeJS.ProcessEnv = {}) {
	const options: SpawnOptions = { cwd: f.worktree, env: { ...f.env, ...env } };
	const child = spawn("bash", [SCRIPT, ...args], options);
	let stderr = "";
	child.stderr?.on("data", (chunk: Buffer) => {
		stderr += chunk.toString();
	});
	const done = new Promise<Run>((resolveRun) => {
		child.on("close", (code) => resolveRun({ code, stderr }));
	});
	return { child, done };
}

function run(f: Fixture, args: string[], env: NodeJS.ProcessEnv = {}): Promise<Run> {
	return start(f, args, env).done;
}

function log(f: Fixture): string[] {
	return existsSync(f.log) ? readFileSync(f.log, "utf8").trim().split("\n") : [];
}

function escapeRegExp(text: string): string {
	return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function until(check: () => boolean): Promise<void> {
	for (let i = 0; i < 200 && !check(); i++) await new Promise((r) => setTimeout(r, 50));
	expect(check()).toBe(true);
}

describe("just shared-build", () => {
	it("refuses by name without an absolute PIJ_SHARED_TARGET and runs no cargo", async () => {
		const f = fixture();
		const unset = await run(f, ["--", "build"], { PIJ_SHARED_TARGET: "" });
		expect(unset.code).toBe(2);
		expect(unset.stderr).toContain("PIJ_SHARED_TARGET is not set");
		const relative = await run(f, ["--", "build"], { PIJ_SHARED_TARGET: "target" });
		expect(relative.code).toBe(2);
		expect(relative.stderr).toContain("must be absolute");
		expect(log(f)).toEqual([]);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("holds one lock for a --then batch, with the shared env, an owner line and fresh sources", async () => {
		const f = fixture();
		const lib = join(f.worktree, "crates", "alpha", "src", "lib.rs");
		utimesSync(lib, 1_000_000, 1_000_000);
		const result = await run(f, [
			"--",
			"build",
			"-p",
			"pij-core",
			"--then",
			"test",
			"-p",
			"pij-core",
		]);
		expect(result.code).toBe(0);
		const lines = log(f);
		expect(lines.filter((line) => line.startsWith("start"))).toEqual([
			`start build target=${f.target} incremental=0`,
			`start test target=${f.target} incremental=0`,
		]);
		const owners = lines.filter((line) => line.startsWith("seat="));
		expect(owners).toHaveLength(2);
		expect(owners[0]).toBe(owners[1]);
		expect(owners[0]).toMatch(
			new RegExp(
				`^seat=pij-test-seat pid=\\d+ worktree=${escapeRegExp(f.worktree)} started=\\d{4}-\\d\\d-\\d\\dT`,
			),
		);
		expect(statSync(lib).mtimeMs).toBeGreaterThan(1_000_000_000);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("releases the lock when cargo fails and runs nothing after the failure", async () => {
		const f = fixture();
		const result = await run(f, ["--", "fail", "--then", "build"]);
		expect(result.code).toBe(7);
		expect(log(f).filter((line) => line.startsWith("start"))).toHaveLength(1);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("releases the lock on SIGTERM while cargo runs", async () => {
		const f = fixture();
		const { child, done } = start(f, ["--", "build"], { NAP: "30" });
		await until(() => log(f).some((line) => line.startsWith("start build")));
		expect(existsSync(f.lock)).toBe(true);
		child.kill("SIGTERM");
		const result = await done;
		expect(result.code).toBe(143);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("refuses under the disk floor before the lock, and releases if the floor is crossed while holding", async () => {
		const f = fixture();
		writeFileSync(f.free, String(10 * GIB_KIB));
		const before = await run(f, ["--", "build"]);
		expect(before.code).toBe(3);
		expect(before.stderr).toMatch(
			/disk floor: 10\.00 GiB free .* below PIJ_DISK_FLOOR_GB=25; no cargo run/,
		);
		expect(log(f)).toEqual([]);
		expect(existsSync(f.lock)).toBe(false);

		writeFileSync(f.free, String(40 * GIB_KIB));
		const during = await run(f, ["--", "drop", "--then", "build"]);
		expect(during.code).toBe(3);
		expect(log(f).filter((line) => line.startsWith("start"))).toEqual([
			`start drop target=${f.target} incremental=0`,
		]);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("never overlaps two concurrent batches: the second waits for the first to release", async () => {
		const f = fixture();
		const first = start(f, ["--", "first"], { NAP: "2" });
		await until(() => log(f).some((line) => line.startsWith("start first")));
		const second = start(f, ["--", "second"], { NAP: "1" });
		const [a, b] = await Promise.all([first.done, second.done]);
		expect([a.code, b.code]).toEqual([0, 0]);
		expect(
			log(f)
				.filter((line) => /^(start|end) /.test(line))
				.map((line) => line.split(" ").slice(0, 2).join(" ")),
		).toEqual(["start first", "end first", "start second", "end second"]);
		expect(b.stderr).toContain(`waiting for ${f.lock}, held by: seat=pij-test-seat`);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("gives up by name after --timeout and leaves a live owner's lock alone", async () => {
		const f = fixture();
		mkdirSync(f.lock);
		writeFileSync(
			join(f.lock, "owner"),
			`seat=pij-other pid=${process.pid} worktree=/elsewhere started=now\n`,
		);
		const result = await run(f, ["--timeout", "2s", "--", "build"]);
		expect(result.code).toBe(4);
		expect(result.stderr).toContain("gave up after --timeout 2s");
		expect(log(f)).toEqual([]);
		expect(existsSync(join(f.lock, "owner"))).toBe(true);
	});

	it("reports a dead owner's lock as stale with the clear command, and never clears it", async () => {
		const f = fixture();
		const gone = spawnSync("sh", ["-c", "echo $$"]).stdout.toString().trim();
		mkdirSync(f.lock);
		writeFileSync(
			join(f.lock, "owner"),
			`seat=pij-gone pid=${gone} worktree=/elsewhere started=then\n`,
		);
		const result = await run(f, ["--", "build"]);
		expect(result.code).toBe(5);
		expect(result.stderr).toContain(
			`stale lock ${f.lock}: its owner is gone (seat=pij-gone pid=${gone}`,
		);
		expect(result.stderr).toContain(`clear it with: rm -rf '${f.lock}'`);
		expect(log(f)).toEqual([]);
		expect(existsSync(join(f.lock, "owner"))).toBe(true);
	});
});
