import { type SpawnOptions, spawn, spawnSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	realpathSync,
	rmSync,
	statSync,
	symlinkSync,
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
/**
 * Git that reads no user or system config: a global `trace2.eventtarget`
 * hands every git command to a machine-wide daemon (git-ai), which then
 * writes into the fixture's .git and races teardown (ENOTEMPTY).
 */
const HERMETIC_GIT = { GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" };

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
	spawnSync("git", ["init", "-q", worktree], { env: { ...process.env, ...HERMETIC_GIT } });
	const target = join(root, "target");
	const log = join(root, "cargo.log");
	const free = join(root, "free-kib");
	writeFileSync(free, String(40 * GIB_KIB));
	// Logs each run's start and end, its environment and the lock owner seen
	// while it runs; `drop` lowers the free space, `fail` exits non-zero.
	executable(
		join(bin, "cargo"),
		[
			// cargo 1.98 refuses `--doc` with `--no-run`: doctests build as they run.
			'case " $* " in *" --doc "*) case " $* " in *" --no-run "*) echo "error: can\'t skip running doc tests with --no-run" >&2; exit 101;; esac;; esac',
			'echo "start $1 target=$CARGO_TARGET_DIR incremental=$CARGO_INCREMENTAL" >> "$LOG"',
			'cat "$CARGO_TARGET_DIR.lock/owner" >> "$LOG"',
			// The parallelism cap that reached cargo (F126).
			'echo "jobs $1 build=$CARGO_BUILD_JOBS test=$RUST_TEST_THREADS" >> "$LOG"',
			'[ "$1" = drop ] && echo 1024 > "$FREE"',
			// The scheduling cargo runs under: its niceness, and the flag a fake
			// taskpolicy passed down, had one wrapped it (F186: none ever does).
			'[ -n "$NICENESS" ] && echo "niceness=$(ps -o nice= -p $$ | tr -d \' \') taskpolicy=$FAKE_TASKPOLICY args=$*" >> "$LOG"',
			// The script's own niceness (nice execs into cargo, so the parent is it).
			'[ -n "$NICENESS" ] && echo "parent-niceness=$(ps -o nice= -p $PPID | tr -d \' \')" >> "$LOG"',
			// A build child (rustc, a build script) that ticks into the log
			// while alive; cargo waits for it, as cargo waits for rustc.
			'[ -n "$WORKER" ] && ( i=0; while [ $i -lt 25 ]; do echo "tick $1" >> "$LOG"; sleep 0.2; i=$((i+1)); done ) </dev/null >/dev/null 2>&1 &',
			// An orphaned sleep must not hold the test's stderr pipe open.
			'[ -n "$NAP" ] && sleep "$NAP" </dev/null >/dev/null 2>&1',
			// A test binary that never ends (F273): `HANG` names it; it runs only
			// for `cargo test`, in cargo's group, as libtest's binary does.
			'[ -n "$HANG" ] && [ "$1" = test ] && "$HANG" </dev/null >/dev/null 2>&1',
			// An artifact in the profile's deps, as a real build leaves (F243).
			// `ARTIFACT_AT` puts it elsewhere, relative to the target (a target triple's layout).
			'[ -n "$ARTIFACT_AT" ] || ARTIFACT_AT=debug/deps/libalpha.rlib',
			'[ -n "$ARTIFACT" ] && at="$CARGO_TARGET_DIR/$ARTIFACT_AT" && mkdir -p "$(dirname "$at")" && date > "$at"',
			"wait",
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
			...HERMETIC_GIT,
			PATH: `${bin}:${process.env.PATH}`,
			PIJ_SHARED_TARGET: target,
			PIJ_SHARED_BUILD_DF: df,
			PIJ_SHARED_BUILD_POLL_SECONDS: "1",
			PIJ_DISK_FLOOR_GB: "25",
			// Unset unless a test supplies them: the caps default to 4.
			PIJ_CARGO_JOBS: "",
			RUST_TEST_THREADS: "",
			// Absent, not empty, so only the script's own export can set it (an
			// ambient one would reach cargo, since bash keeps it exported).
			CARGO_BUILD_JOBS: undefined,
			PIJ_ID: "pij-test-seat",
			LOG: log,
			FREE: free,
			NAP: "",
			WORKER: "",
			NICENESS: "",
			HANG: "",
			HANG_PID: join(root, "hang.pid"),
			PIJ_SHARED_BUILD_TEST_LIMIT: undefined,
			PIJ_TEST_TIME_SCALE: undefined,
			ARTIFACT: "",
			ARTIFACT_AT: "",
			FAKE_TASKPOLICY: "none",
		},
	};
}

interface Run {
	code: number | null;
	stderr: string;
}

function start(
	f: Fixture,
	args: string[],
	env: NodeJS.ProcessEnv = {},
	umask?: string,
	parentNice: readonly number[] = [],
) {
	const options: SpawnOptions = { cwd: f.worktree, env: { ...f.env, ...env } };
	// The caller's own niceness: each entry wraps the script in one more nice.
	const niced = parentNice.flatMap((n) => ["/usr/bin/nice", "-n", String(n)]);
	const child = umask
		? spawn("sh", ["-c", `umask ${umask} && exec bash "$0" "$@"`, SCRIPT, ...args], options)
		: niced.length > 0
			? spawn(niced[0] ?? "", [...niced.slice(1), "bash", SCRIPT, ...args], options)
			: spawn("bash", [SCRIPT, ...args], options);
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

	it("caps cargo's jobs and test threads at 4 by default, prints the cap, and PIJ_CARGO_JOBS sets both (F126)", async () => {
		const jobs = (f: Fixture) => log(f).filter((line) => line.startsWith("jobs "));
		const capped = fixture();
		const result = await run(capped, ["--", "build", "--then", "test"]);
		expect(result.code).toBe(0);
		expect(result.stderr).toContain(
			"shared-build: cargo jobs: 4 (PIJ_CARGO_JOBS), test threads: 4 (RUST_TEST_THREADS)",
		);
		// The build and the test (one step, F186) both see the cap.
		expect(jobs(capped)).toEqual(["jobs build build=4 test=4", "jobs test build=4 test=4"]);

		const two = fixture();
		const set = await run(two, ["--", "build"], { PIJ_CARGO_JOBS: "2" });
		expect(set.stderr).toContain("cargo jobs: 2 (PIJ_CARGO_JOBS), test threads: 2");
		expect(jobs(two)).toEqual(["jobs build build=2 test=2"]);

		// A caller's own RUST_TEST_THREADS wins over the default test cap.
		const own = fixture();
		await run(own, ["--", "test"], { PIJ_CARGO_JOBS: "6", RUST_TEST_THREADS: "1" });
		expect(jobs(own)).toEqual(["jobs test build=6 test=1"]);
	});

	it("refuses a PIJ_CARGO_JOBS that is not a positive whole number, before the lock and any cargo", async () => {
		for (const bad of ["0", "four", "-2"]) {
			const f = fixture();
			const result = await run(f, ["--", "build"], { PIJ_CARGO_JOBS: bad });
			expect(result.code).toBe(2);
			expect(result.stderr).toContain("PIJ_CARGO_JOBS must be a positive whole number");
			expect(log(f)).toEqual([]);
			expect(existsSync(f.lock)).toBe(false);
		}
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
		// One cargo call per command (F186: no --no-run compile step).
		expect(lines.filter((line) => line.startsWith("start"))).toEqual([
			`start build target=${f.target} incremental=0`,
			`start test target=${f.target} incremental=0`,
		]);
		const owners = lines.filter((line) => line.startsWith("seat="));
		expect(owners).toHaveLength(2);
		expect(new Set(owners).size).toBe(1);
		expect(owners[0]).toMatch(
			new RegExp(
				`^seat=pij-test-seat pid=\\d+ worktree=${escapeRegExp(f.worktree)} started=\\d{4}-\\d\\d-\\d\\dT`,
			),
		);
		expect(statSync(lib).mtimeMs).toBeGreaterThan(1_000_000_000);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("touches this tree's crate roots only after another tree, or a build it did not finish, wrote the target (F243)", async () => {
		const f = fixture();
		const lib = join(f.worktree, "crates", "alpha", "src", "lib.rs");
		const touched = async (from: Fixture, args = ["--", "build"]) => {
			utimesSync(lib, 1_000_000, 1_000_000);
			await run(from, args, { ARTIFACT: "1" });
			return statSync(lib).mtimeMs > 1_000_000_000;
		};
		// Nothing recorded yet: this tree's sources are made newer than any artifact.
		expect(await touched(f)).toBe(true);
		// This tree again, and nothing else built since: cargo's own mtimes hold.
		expect(await touched(f)).toBe(false);
		expect(await touched(f)).toBe(false);
		// Another worktree builds into the same target: this tree's next run touches.
		const other = join(f.root, "other");
		mkdirSync(join(other, "crates", "alpha", "src"), { recursive: true });
		writeFileSync(join(other, "crates", "alpha", "src", "lib.rs"), "\n");
		spawnSync("git", ["init", "-q", other], { env: { ...process.env, ...HERMETIC_GIT } });
		await run({ ...f, worktree: other }, ["--", "build"], { ARTIFACT: "1" });
		expect(await touched(f)).toBe(true);
		expect(await touched(f)).toBe(false);
		// A bare cargo build wrote an artifact after the last finished run.
		const bare = join(f.target, "debug", "deps", "libbare.rlib");
		writeFileSync(bare, "x");
		const later = Date.now() / 1000 + 60;
		utimesSync(bare, later, later);
		expect(await touched(f)).toBe(true);
		rmSync(bare);
		// A failed run records nothing, so its artifacts count as another build.
		expect(await touched(f, ["--", "fail"])).toBe(false);
		expect(await touched(f)).toBe(true);
	});

	it("never writes the stamp through a link or into a directory planted at its path, and then always touches", async () => {
		const f = fixture();
		const lib = join(f.worktree, "crates", "alpha", "src", "lib.rs");
		const elsewhere = join(f.root, "elsewhere");
		mkdirSync(elsewhere);
		mkdirSync(f.target);
		symlinkSync(elsewhere, join(f.target, ".pij-shared-build-tree"));
		for (let i = 0; i < 2; i++) {
			utimesSync(lib, 1_000_000, 1_000_000);
			expect((await run(f, ["--", "build"], { ARTIFACT: "1" })).code).toBe(0);
			expect(statSync(lib).mtimeMs).toBeGreaterThan(1_000_000_000);
		}
		expect(readdirSync(elsewhere)).toEqual([]);
		expect(lstatSync(join(f.target, ".pij-shared-build-tree")).isSymbolicLink()).toBe(true);
		expect(
			readdirSync(f.target).filter((name) => name.startsWith(".pij-shared-build-tree.")),
		).toEqual([]);
	});

	it("touches after another tree's batch built and then failed, in any cargo layout (R297-001)", async () => {
		const f = fixture();
		const lib = join(f.worktree, "crates", "alpha", "src", "lib.rs");
		const other = join(f.root, "other");
		mkdirSync(join(other, "crates", "alpha", "src"), { recursive: true });
		writeFileSync(join(other, "crates", "alpha", "src", "lib.rs"), "\n");
		spawnSync("git", ["init", "-q", other], { env: { ...process.env, ...HERMETIC_GIT } });
		const touchedAfter = async (env: NodeJS.ProcessEnv, from: Fixture, args: string[]) => {
			await run(f, ["--", "build"], { ARTIFACT: "1" });
			utimesSync(lib, 1_000_000, 1_000_000);
			await run(f, ["--", "build"], { ARTIFACT: "1" });
			expect(statSync(lib).mtimeMs).toBe(1_000_000_000);
			await run(from, args, { ARTIFACT: "1", ...env });
			await run(f, ["--", "build"]);
			return statSync(lib).mtimeMs > 1_000_000_000;
		};
		// termite's case: another tree's build succeeds into an explicit
		// --target layout, then a later command of the same batch fails.
		const triple = { ARTIFACT_AT: "aarch64-apple-darwin/debug/deps/libalpha.rlib" };
		expect(
			await touchedAfter(triple, { ...f, worktree: other }, ["--", "build", "--then", "fail"]),
		).toBe(true);
		// The same failed batch, whatever it wrote and wherever (here no deps file).
		const binary = { ARTIFACT_AT: "debug/pij-rs" };
		expect(
			await touchedAfter(binary, { ...f, worktree: other }, ["--", "build", "--then", "fail"]),
		).toBe(true);
		// A round-1 stamp (bare `<tree>`) that a failed batch left trusted and
		// newer than the other tree's artifacts (snail): each tree touches once
		// on adoption, then trusts its own finished stamp again.
		await run({ ...f, worktree: other }, ["--", "build", "--then", "fail"], {
			...triple,
			ARTIFACT: "1",
		});
		const legacy = join(f.target, ".pij-shared-build-tree");
		writeFileSync(legacy, `${f.worktree}\n`);
		const ahead = Date.now() / 1000 + 120;
		utimesSync(legacy, ahead, ahead);
		utimesSync(lib, 1_000_000, 1_000_000);
		await run(f, ["--", "build"]);
		expect(statSync(lib).mtimeMs).toBeGreaterThan(1_000_000_000);
		utimesSync(lib, 1_000_000, 1_000_000);
		await run(f, ["--", "build"]);
		expect(statSync(lib).mtimeMs).toBe(1_000_000_000);
		// Bare cargo outside any run, into a target triple's deps.
		utimesSync(lib, 1_000_000, 1_000_000);
		await run(f, ["--", "build"], { ARTIFACT: "1" });
		const bare = join(f.target, "aarch64-apple-darwin", "release", "deps", "libbare.rlib");
		mkdirSync(join(bare, ".."), { recursive: true });
		writeFileSync(bare, "x");
		const later = Date.now() / 1000 + 60;
		utimesSync(bare, later, later);
		utimesSync(lib, 1_000_000, 1_000_000);
		await run(f, ["--", "build"]);
		expect(statSync(lib).mtimeMs).toBeGreaterThan(1_000_000_000);
	});

	it("removes a stamp it cannot replace, so another tree's build never leaves an earlier one trusted (F246)", async () => {
		const f = fixture();
		const lib = join(f.worktree, "crates", "alpha", "src", "lib.rs");
		const other = join(f.root, "other");
		mkdirSync(join(other, "crates", "alpha", "src"), { recursive: true });
		writeFileSync(join(other, "crates", "alpha", "src", "lib.rs"), "\n");
		spawnSync("git", ["init", "-q", other], { env: { ...process.env, ...HERMETIC_GIT } });
		const stamp = join(f.target, ".pij-shared-build-tree");
		const noMktemp = join(f.root, "no-mktemp");
		executable(noMktemp, "exit 1");
		// This tree finished here; then the other tree builds a binary outside
		// any deps (so no scan sees it) and can't write its stamps.
		expect((await run(f, ["--", "build"], { ARTIFACT: "1" })).code).toBe(0);
		const failed = await run({ ...f, worktree: other }, ["--", "build"], {
			ARTIFACT: "1",
			ARTIFACT_AT: "debug/pij-rs",
			PIJ_SHARED_BUILD_MKTEMP: noMktemp,
		});
		expect(failed.code).toBe(0);
		utimesSync(lib, 1_000_000, 1_000_000);
		await run(f, ["--", "build"]);
		expect(statSync(lib).mtimeMs).toBeGreaterThan(1_000_000_000);
		expect(failed.stderr).toContain(`could not write ${stamp}; removed it`);
		// A stamp that can be neither replaced nor removed refuses before cargo.
		const before = readFileSync(f.log, "utf8");
		chmodSync(f.target, 0o555);
		try {
			const refused = await run({ ...f, worktree: other }, ["--", "build"], {
				PIJ_SHARED_BUILD_MKTEMP: noMktemp,
			});
			expect(refused.code).toBe(7);
			expect(refused.stderr).toContain(`could not write or remove ${stamp}`);
		} finally {
			chmodSync(f.target, 0o755);
		}
		expect(readFileSync(f.log, "utf8")).toBe(before);
		expect(readFileSync(stamp, "utf8")).toBe(`finished ${f.worktree}\n`);
	});

	it("names the seat in the owner line from PIJ_SESSION_ID or whoami when PIJ_ID is unset, else the user", async () => {
		const f = fixture();
		const whoami = join(f.root, "fake-whoami");
		writeFileSync(
			whoami,
			'#!/bin/sh\n[ "$1 $2" = "whoami --json" ] && echo \'{"ok":true,"data":{"id":"pij-whoami-seat","proc":{"id":"x"}}}\'\n',
		);
		chmodSync(whoami, 0o755);
		const silent = join(f.root, "silent-whoami");
		writeFileSync(silent, "#!/bin/sh\nexit 1\n");
		chmodSync(silent, 0o755);
		const owner = async (env: NodeJS.ProcessEnv) => {
			writeFileSync(f.log, "");
			expect((await run(f, ["--", "build"], env)).code).toBe(0);
			return log(f)
				.find((line) => line.startsWith("seat="))
				?.split(" ")[0];
		};
		const unset = { PIJ_ID: undefined, PIJ_SESSION_ID: undefined };
		expect(await owner({ ...unset, PIJ_SESSION_ID: "pij-session-seat" })).toBe(
			"seat=pij-session-seat",
		);
		expect(await owner({ ...unset, PIJ_SHARED_BUILD_WHOAMI: whoami })).toBe("seat=pij-whoami-seat");
		expect(await owner({ ...unset, PIJ_SHARED_BUILD_WHOAMI: silent, USER: "someone" })).toBe(
			"seat=someone",
		);
	});

	it("runs every cargo command under nice 19 alone, as given, never in taskpolicy (F186)", async () => {
		const f = fixture();
		// A taskpolicy to find, on PATH and at the retired seam: nothing may wrap
		// cargo in it (Jordan's ruling of 2026-11-11 dropped background QoS).
		const taskpolicy = join(f.root, "bin", "taskpolicy");
		executable(taskpolicy, 'FAKE_TASKPOLICY="$1"; export FAKE_TASKPOLICY; shift; exec "$@"');
		const result = await run(
			f,
			[
				"--",
				"build",
				"--then",
				"test",
				"-p",
				"x",
				"--",
				"name",
				"--then",
				"test",
				"-p",
				"x",
				"--doc",
				"--",
				"name",
			],
			{ NICENESS: "1", PIJ_SHARED_BUILD_TASKPOLICY: taskpolicy },
		);
		// Nothing but the cap line (F126): no cargo refusal of `--doc --no-run`.
		expect(result.stderr).toBe(
			"shared-build: cargo jobs: 4 (PIJ_CARGO_JOBS), test threads: 4 (RUST_TEST_THREADS)\n",
		);
		expect(result.code).toBe(0);
		// One step per command: a test is no longer split into a --no-run compile
		// and a run (F63's split fell away with background QoS). Each runs at
		// max(19, the runner's niceness): 19 under bare `npm test` or `just test`,
		// 20 under an outer nice on a recipe that already nices (F220).
		const runner = Number(
			log(f)
				.find((line) => line.startsWith("parent-niceness="))
				?.slice("parent-niceness=".length),
		);
		const niceness = Math.max(19, runner);
		expect(log(f).filter((line) => line.startsWith("niceness="))).toEqual([
			`niceness=${niceness} taskpolicy=none args=build`,
			`niceness=${niceness} taskpolicy=none args=test -p x -- name`,
			`niceness=${niceness} taskpolicy=none args=test -p x --doc -- name`,
		]);
		expect(existsSync(f.lock)).toBe(false);
	});

	// R257-001: `just test` runs vitest under nice -n 19, so this script ran with a
	// niced parent and its own nice -n 19 stacked cargo to 20. Cargo ends at the
	// script's niceness or 19, whichever is higher: below 19 it is raised to 19,
	// and at 19 or above nothing is added. The test runner's own niceness (0 in
	// CI, 19 under `just test`) is why the script's is read, not assumed.
	it.each([
		[[5]],
		[[19]],
		[[19, 19]],
	] as const)("never stacks nice on a parent already niced by %j", async (parentNice) => {
		const f = fixture();
		const result = await start(f, ["--", "build"], { NICENESS: "1" }, undefined, parentNice).done;
		expect(result.code, result.stderr).toBe(0);
		const value = (prefix: string): number =>
			Number(
				log(f)
					.find((line) => line.startsWith(prefix))
					?.slice(prefix.length)
					.split(" ")[0],
			);
		const script = value("parent-niceness=");
		expect(script).toBeGreaterThanOrEqual(Math.min(...parentNice));
		expect(value("niceness=")).toBe(Math.max(19, script));
	});

	it("releases the lock when cargo fails and runs nothing after the failure", async () => {
		const f = fixture();
		const result = await run(f, ["--", "fail", "--then", "build"]);
		expect(result.code).toBe(7);
		expect(log(f).filter((line) => line.startsWith("start"))).toHaveLength(1);
		expect(existsSync(f.lock)).toBe(false);
	});

	for (const [signal, code] of [
		["SIGTERM", 143],
		["SIGINT", 130],
		["SIGHUP", 129],
		// Untrapped: bash dies of it, but runs its EXIT trap first (F48).
		["SIGUSR1", null],
	] as const) {
		it(`on ${signal}, stops cargo's whole process tree before another batch can enter`, async () => {
			const f = fixture();
			const first = start(f, ["--", "first"], { WORKER: "1" });
			await until(() => log(f).some((line) => line === "tick first"));
			expect(existsSync(f.lock)).toBe(true);
			first.child.kill(signal);
			const stopped = await first.done;
			expect(stopped.code).toBe(code);
			expect(existsSync(f.lock)).toBe(false);
			// The worker would tick for 5 s more if it had survived its cargo.
			const second = await run(f, ["--", "second"], { NAP: "1.5" });
			expect(second.code).toBe(0);
			const lines = log(f);
			const entered = lines.indexOf(`start second target=${f.target} incremental=0`);
			expect(entered).toBeGreaterThan(0);
			expect(lines.slice(entered).filter((line) => line.includes("first"))).toEqual([]);
			expect(lines).not.toContain("end first");
		});
	}

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

	it("stops a `cargo test` past its limit by name, in its own process group only, and releases the lock (F273)", async () => {
		const f = fixture();
		// The hung test binary, and a bystander running the same file outside
		// the run's group: only the run's own group is ever stopped.
		const hung = join(f.root, "bin", "pij_rs-0123abcd");
		executable(hung, 'echo $$ > "$HANG_PID"\n/bin/sleep 600');
		const bystander = spawn(hung, [], {
			detached: true,
			stdio: "ignore",
			env: { ...f.env, HANG_PID: join(f.root, "bystander.pid") },
		});
		const started = Date.now();
		const result = await run(f, ["--", "test", "--then", "build"], {
			HANG: hung,
			PIJ_SHARED_BUILD_TEST_LIMIT: "1s",
			PIJ_TEST_TIME_SCALE: "2",
		});
		expect(result.code).toBe(8);
		expect(result.stderr).toContain(
			"cargo test ran past its test limit of 2s (PIJ_SHARED_BUILD_TEST_LIMIT=1s, PIJ_TEST_TIME_SCALE=2)",
		);
		expect(result.stderr).toContain("pij_rs-0123abcd");
		expect(result.stderr).toContain("/bin/sleep 600");
		expect(result.stderr).toContain("stopping that process group, and only it");
		expect(Date.now() - started).toBeLessThan(15_000);
		expect(existsSync(f.lock)).toBe(false);
		expect(log(f).filter((line) => line.startsWith("start"))).toEqual([
			`start test target=${f.target} incremental=0`,
		]);
		const hungPid = Number(readFileSync(join(f.root, "hang.pid"), "utf8"));
		expect(() => process.kill(hungPid, 0)).toThrow();
		const bystanderPid = bystander.pid;
		if (bystanderPid === undefined) throw new Error("the bystander never started");
		expect(() => process.kill(bystanderPid, 0)).not.toThrow();
		// Its own group (detached): the bystander and its sleep, nothing else.
		process.kill(-bystanderPid, "SIGKILL");
	});

	it("limits only `cargo test`, never past a finished run, and refuses a malformed limit before the lock (F273)", async () => {
		const f = fixture();
		const quick = await run(f, ["--", "test", "--then", "build"], {
			PIJ_SHARED_BUILD_TEST_LIMIT: "1s",
			NAP: "0.2",
		});
		expect(quick.code).toBe(0);
		expect(quick.stderr).not.toContain("ran past");
		const slowBuild = await run(f, ["--", "build"], {
			PIJ_SHARED_BUILD_TEST_LIMIT: "1s",
			NAP: "1.5",
		});
		expect(slowBuild.code).toBe(0);
		const none = await run(f, ["--", "test"], { PIJ_SHARED_BUILD_TEST_LIMIT: "0", NAP: "1.5" });
		expect(none.code).toBe(0);
		for (const [name, value] of [
			["PIJ_SHARED_BUILD_TEST_LIMIT", "soon"],
			["PIJ_TEST_TIME_SCALE", "0.5"],
		]) {
			const before = log(f).length;
			const bad = await run(f, ["--", "test"], { [name]: value });
			expect(bad.code).toBe(2);
			expect(bad.stderr).toContain(name);
			expect(log(f)).toHaveLength(before);
			expect(existsSync(f.lock)).toBe(false);
		}
	});

	it("takes one lock for a symlink and its real directory, and builds into the physical path", async () => {
		const f = fixture();
		mkdirSync(f.target);
		const alias = join(f.root, "alias");
		symlinkSync(f.target, alias);
		const first = start(f, ["--", "first"], { NAP: "2" });
		await until(() => log(f).some((line) => line.startsWith("start first")));
		const second = await run(f, ["--", "second"], { PIJ_SHARED_TARGET: alias, NAP: "0.2" });
		expect((await first.done).code).toBe(0);
		expect(second.code).toBe(0);
		expect(second.stderr).toContain(`waiting for ${f.lock}`);
		expect(log(f).filter((line) => /^(start|end) /.test(line))).toEqual([
			`start first target=${f.target} incremental=0`,
			"end first",
			`start second target=${f.target} incremental=0`,
			"end second",
		]);
		const later = join(f.target, "not-yet", "built");
		const missing = await run(f, ["--", "third"], {
			PIJ_SHARED_TARGET: join(alias, "not-yet", "built"),
		});
		expect(missing.code).toBe(0);
		expect(log(f)).toContain(`start third target=${later} incremental=0`);
		expect(existsSync(`${later}.lock`)).toBe(false);
	});

	it("takes one lock for `target/` and `target` while the target does not exist yet", async () => {
		const f = fixture();
		expect(existsSync(f.target)).toBe(false);
		const first = start(f, ["--", "first"], { PIJ_SHARED_TARGET: `${f.target}/`, NAP: "2" });
		await until(() => log(f).some((line) => line.startsWith("start first")));
		const second = await run(f, ["--", "second"], { PIJ_SHARED_TARGET: f.target, NAP: "0.2" });
		expect((await first.done).code).toBe(0);
		expect(second.code).toBe(0);
		expect(second.stderr).toContain(`waiting for ${f.lock}`);
		expect(log(f).filter((line) => /^(start|end) /.test(line))).toEqual([
			`start first target=${f.target} incremental=0`,
			"end first",
			`start second target=${f.target} incremental=0`,
			"end second",
		]);
		expect(existsSync(join(f.target, ".lock"))).toBe(false);
	});

	it("under a permissive umask (002) the lock is private: 0700 while held", async () => {
		const f = fixture();
		const held = start(f, ["--", "first"], { NAP: "1.5" }, "002");
		await until(() => log(f).some((line) => line.startsWith("start first")));
		expect(statSync(f.lock).mode & 0o777).toBe(0o700);
		expect((await held.done).code).toBe(0);
		expect(existsSync(f.lock)).toBe(false);
	});

	it("refuses an owner path planted after the lock is made, and never follows it", async () => {
		const f = fixture();
		const victim = join(f.root, "victim");
		writeFileSync(victim, "keep\n");
		const plant = join(f.root, "planting-mkdir");
		executable(
			plant,
			[
				'/bin/mkdir "$@" || exit $?',
				"for last; do :; done",
				`ln -s "${victim}" "$last/owner"`,
			].join("\n"),
		);
		const result = await start(f, ["--", "build"], { PIJ_SHARED_BUILD_MKDIR: plant }, "002").done;
		expect(result.code).toBe(2);
		expect(result.stderr).toContain(`something already sits at ${f.lock}/owner; no cargo run`);
		expect(readFileSync(victim, "utf8")).toBe("keep\n");
		expect(lstatSync(victim).isFile()).toBe(true);
		expect(log(f)).toEqual([]);
		expect(existsSync(f.lock)).toBe(false);
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

	// F226, R271-001: the deploy build installs the bytes it built, so the copy
	// is taken before the lock lets another tree build into the target.
	it("copies an artifact out only after every command succeeded, then releases", async () => {
		const f = fixture();
		mkdirSync(join(f.target, "release"), { recursive: true });
		writeFileSync(join(f.target, "release", "pij-rs"), "built\n");
		const dest = join(f.root, "copy");
		const ok = await run(f, ["--copy-out", "release/pij-rs", dest, "--", "build"]);
		expect(ok.code, ok.stderr).toBe(0);
		expect(readFileSync(dest, "utf8")).toBe("built\n");
		expect(existsSync(f.lock)).toBe(false);
		const again = await run(f, ["--copy-out", "release/pij-rs", dest, "--", "build"]);
		expect(again.code, "an existing destination is never overwritten").toBe(6);
		expect(existsSync(f.lock)).toBe(false);
		const never = join(f.root, "never");
		const failed = await run(f, ["--copy-out", "release/pij-rs", never, "--", "fail"]);
		expect(failed.code).toBe(7);
		expect(existsSync(never), "a failed build copies nothing").toBe(false);
	});
});
