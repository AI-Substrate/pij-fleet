import { spawn, spawnSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// F226: the one deploy build, driven with a fake cargo in temporary
// directories. Never the real target, the installed pij-rs or the daemon.
const SCRIPT = resolve(import.meta.dirname, "release-build.sh");
const INSTALL = resolve(import.meta.dirname, "install-pij-rs.sh");
const roots: string[] = [];

afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function executable(path: string, body: string): void {
	writeFileSync(path, `#!/bin/sh\n${body}\n`);
	chmodSync(path, 0o755);
}

/** A built pij-rs that answers `--version` with `stamp`. */
function binary(path: string, stamp: string): void {
	executable(path, `echo "pij-rs 0.1.0 (${stamp})"`);
}

interface Fixture {
	root: string;
	repo: string;
	home: string;
	tmp: string;
	target: string;
	log: string;
	commit: string;
	env: NodeJS.ProcessEnv;
}

/**
 * Git that reads no user or system config. A global `trace2.eventtarget` hands
 * every git command to a machine-wide daemon (git-ai), which then writes into
 * the fixture's .git after the command returned: teardown raced it and failed
 * ENOTEMPTY. The script under test runs its git with the same environment.
 */
const HERMETIC_GIT = { GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" };

const git = (repo: string, ...args: string[]) =>
	spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@t", ...args], {
		cwd: repo,
		encoding: "utf8",
		env: { ...process.env, ...HERMETIC_GIT },
	});

/** A checkout with the paths the stamp names; `marker` makes its commit its own. */
function checkout(root: string, name: string, marker: string): { repo: string; commit: string } {
	const repo = join(root, name);
	for (const file of [
		".copilot/extensions/pij/extension.mjs",
		".omp/extensions/pij/index.ts",
		"crates/core/tests/fixtures/control-commands.json",
		"crates/core/src/lib.rs",
		".omp/models.yml",
		// R271-002: embedded by crates from outside crates/.
		"harness/scripts/claude-stop-pij.sh",
		".dd/schemas/pij/role/schema.json",
		"templates/roles/pm/v1/role.dd.json",
		// Not built in: a script no crate embeds.
		"harness/scripts/unrelated.sh",
	]) {
		mkdirSync(join(repo, file, ".."), { recursive: true });
		writeFileSync(join(repo, file), `${file} ${marker}\n`);
	}
	git(repo, "init", "-q");
	git(repo, "add", ".");
	git(repo, "commit", "-q", "-m", marker);
	return { repo, commit: git(repo, "rev-parse", "HEAD").stdout.trim().slice(0, 10) };
}

function fixture(): Fixture {
	const root = realpathSync(mkdtempSync(join(tmpdir(), "pij-release-build-")));
	roots.push(root);
	const { repo, commit } = checkout(root, "repo", "a");
	const bin = join(root, "bin");
	mkdirSync(bin);
	const target = join(root, "target");
	mkdirSync(join(target, "release"), { recursive: true });
	// A binary another tree built earlier, unstamped: what the F226 bounce installed.
	binary(join(target, "release", "pij-rs"), "unstamped");
	const log = join(root, "cargo.log");
	// ok: builds a binary carrying the stamp it was given. fail: exits 101 and
	// builds nothing. foreign: succeeds without rebuilding, leaving a binary
	// another tree stamped (cargo trusting newer foreign artifacts).
	executable(
		join(bin, "cargo"),
		[
			'echo "cargo $* target=$CARGO_TARGET_DIR incremental=$CARGO_INCREMENTAL commit=$PIJ_BUILD_COMMIT copilot=$PIJ_EXT_TREE_COPILOT omp=$PIJ_EXT_TREE_OMP" >> "$LOG"',
			'[ -d "$CARGO_TARGET_DIR.lock" ] && echo "locked" >> "$LOG"',
			'case "$FAKE_CARGO" in',
			'ok) printf \'#!/bin/sh\\necho "pij-rs 0.1.0 (%s)"\\n\' "$PIJ_BUILD_COMMIT" > "$CARGO_TARGET_DIR/release/pij-rs" ;;',
			'foreign) printf \'#!/bin/sh\\necho "pij-rs 0.1.0 (0123456789)"\\n\' > "$CARGO_TARGET_DIR/release/pij-rs" ;;',
			"fail) echo 'error[E0308]: mismatched types' >&2; exit 101 ;;",
			// hang: a build still running when the helper is signalled; it ends
			// when the test writes $LOG.release (bounded, so it never outlives one).
			'hang) touch "$LOG.started"; i=0; while [ ! -e "$LOG.release" ] && [ "$i" -lt 600 ]; do sleep 0.05; i=$((i + 1)); done; printf \'#!/bin/sh\\necho "pij-rs 0.1.0 (%s)"\\n\' "$PIJ_BUILD_COMMIT" > "$CARGO_TARGET_DIR/release/pij-rs" ;;',
			"esac",
		].join("\n"),
	);
	const df = join(root, "df");
	executable(
		df,
		[
			'echo "Filesystem 1024-blocks Used Available Capacity Mounted on"',
			'echo "fake 1 0 99999999999 0% /"',
		].join("\n"),
	);
	const home = join(root, "home");
	const tmp = join(root, "tmp");
	mkdirSync(home);
	mkdirSync(tmp);
	return {
		root,
		repo,
		home,
		tmp,
		target,
		log,
		commit,
		env: {
			...process.env,
			...HERMETIC_GIT,
			PATH: `${bin}:${process.env.PATH}`,
			CARGO_TARGET_DIR: target,
			PIJ_SHARED_TARGET: undefined,
			PIJ_SHARED_BUILD_DF: df,
			PIJ_ID: "pij-test-seat",
			PIJ_BUILD_COMMIT: undefined,
			TMPDIR: tmp,
			LOG: log,
			HOME: home,
		},
	};
}

function build(f: Fixture, mode: string, repo = f.repo) {
	return spawnSync("bash", [SCRIPT], {
		cwd: repo,
		env: { ...f.env, FAKE_CARGO: mode },
		encoding: "utf8",
	});
}

/** The deploy recipes' own composition: build, then install what it handed on. */
function deploy(f: Fixture, mode: string) {
	return spawnSync(
		"/bin/sh",
		[
			"-c",
			'set -eu; built="$(bash "$1")"; trap \'rm -rf "${built%/*}"\' EXIT; sh "$2" "$built"',
			"deploy",
			SCRIPT,
			INSTALL,
		],
		{ cwd: f.repo, env: { ...f.env, FAKE_CARGO: mode }, encoding: "utf8" },
	);
}

const installed = (f: Fixture) => join(f.home, ".local/share/pij/bin/pij-rs");
const version = (path: string) => spawnSync(path, ["--version"], { encoding: "utf8" }).stdout;
/** Snapshot directories left in the fixture's TMPDIR. */
const snapshots = (f: Fixture) =>
	readdirSync(f.tmp).filter((name) => name.startsWith("pij-release."));

describe("release-build.sh, the one deploy build (F226)", () => {
	it("builds through shared-build, stamped, and hands on a private copy of this commit's binary", () => {
		const f = fixture();
		const run = build(f, "ok");
		expect(run.status, run.stderr).toBe(0);
		const copy = run.stdout.trim();
		expect(copy).toMatch(new RegExp(`^${f.tmp}/pij-release\\.\\w+/pij-rs$`));
		expect(version(copy)).toBe(`pij-rs 0.1.0 (${f.commit})\n`);
		const log = readFileSync(f.log, "utf8");
		expect(log).toContain(
			`cargo build --release --locked -p pij-cli --bin pij-rs target=${f.target} incremental=0 commit=${f.commit} copilot=`,
		);
		expect(log).toMatch(/omp=\w{10}\+\w{10}/);
		expect(log).toContain("locked");
		rmSync(join(copy, ".."), { recursive: true });
		const done = deploy(f, "ok");
		expect(done.status, done.stderr).toBe(0);
		expect(version(installed(f))).toBe(`pij-rs 0.1.0 (${f.commit})\n`);
		expect(snapshots(f), "the deploy removes its copy").toEqual([]);
	});

	// R271-001: A builds and checks, B builds into the same target, A installs.
	it("installs the bytes it checked, whatever builds into the shared target after", () => {
		const f = fixture();
		const b = checkout(f.root, "repo-b", "b");
		expect(b.commit).not.toBe(f.commit);
		const a = build(f, "ok");
		expect(a.status, a.stderr).toBe(0);
		const other = build(f, "ok", b.repo);
		expect(other.status, other.stderr).toBe(0);
		expect(version(join(f.target, "release", "pij-rs"))).toBe(`pij-rs 0.1.0 (${b.commit})\n`);
		const install = spawnSync("sh", [INSTALL, a.stdout.trim()], { env: f.env, encoding: "utf8" });
		expect(install.status, install.stderr).toBe(0);
		expect(version(installed(f)), "A installs A's binary, never B's").toBe(
			`pij-rs 0.1.0 (${f.commit})\n`,
		);
	});

	it("a failed build exits with cargo's status and installs nothing", () => {
		const f = fixture();
		const run = build(f, "fail");
		expect(run.status).toBe(101);
		expect(run.stdout).toBe("");
		const done = deploy(f, "fail");
		expect(done.status).not.toBe(0);
		expect(existsSync(installed(f)), "the stale binary must never be installed").toBe(false);
		expect(snapshots(f)).toEqual([]);
	});

	it("refuses a binary stamped with another commit, and installs nothing", () => {
		const f = fixture();
		const run = build(f, "foreign");
		expect(run.status).toBe(1);
		expect(run.stdout).toBe("");
		expect(run.stderr).toContain(`not this checkout's commit ${f.commit}`);
		const done = deploy(f, "foreign");
		expect(done.status).not.toBe(0);
		expect(existsSync(installed(f)), "a foreign binary must never be installed").toBe(false);
		expect(snapshots(f)).toEqual([]);
	});

	it("refuses build inputs that differ from the HEAD it stamps; other local changes don't block", () => {
		const f = fixture();
		for (const [file, change] of [
			["crates/core/src/lib.rs", "uncommitted source\n"],
			["crates/core/src/new.rs", "untracked source\n"],
			["harness/scripts/claude-stop-pij.sh", "an edited embedded hook\n"],
			[".dd/schemas/pij/role/schema.json", "{}\n"],
			["templates/roles/pm/v1/role.dd.json", "{}\n"],
			["templates/roles/new/v1/role.dd.json", "an untracked template\n"],
		] as const) {
			const dirty = fixture();
			mkdirSync(join(dirty.repo, file, ".."), { recursive: true });
			writeFileSync(join(dirty.repo, file), change);
			const run = build(dirty, "ok");
			expect(run.status, file).toBe(1);
			expect(run.stdout).toBe("");
			expect(run.stderr).toContain("build inputs differ from HEAD");
			expect(run.stderr).toContain(file);
			expect(existsSync(dirty.log), "nothing was built").toBe(false);
		}
		writeFileSync(join(f.repo, ".omp/models.yml"), "a local model override\n");
		writeFileSync(join(f.repo, "harness/scripts/unrelated.sh"), "not built in\n");
		const run = build(f, "ok");
		expect(run.status, run.stderr).toBe(0);
	});

	// R271-003, R288-FU-01: a helper signalled mid-build keeps its snapshot
	// while the build it waits on still runs (bash runs a trap only once the
	// foreground build returns), then removes it and exits 128 + the signal.
	// Only the helper is signalled, as a parent killing it would. Each signal
	// needs its own handler: without one, HUP kills the helper and leaks the
	// snapshot, and INT lets it carry on and hand the build on.
	it.each([
		["SIGHUP", 129],
		["SIGINT", 130],
		["SIGTERM", 143],
	] as const)("keeps its snapshot through the build, then removes it, on %s", async (signal, code) => {
		const f = fixture();
		const child = spawn("bash", [SCRIPT], {
			cwd: f.repo,
			env: { ...f.env, FAKE_CARGO: "hang" },
			stdio: ["ignore", "pipe", "pipe"],
		});
		const exited = new Promise<number | null>((done) => child.on("exit", (status) => done(status)));
		for (let waited = 0; !existsSync(`${f.log}.started`); waited += 20) {
			expect(waited, "the build started").toBeLessThan(20_000);
			await new Promise((resolve) => setTimeout(resolve, 20));
		}
		expect(snapshots(f)).toHaveLength(1);
		child.kill(signal);
		// Time for a handler that ran at once to have removed it; the build has
		// not been released, so it is still running and reading the snapshot.
		await new Promise((resolve) => setTimeout(resolve, 200));
		expect(snapshots(f), "the running build still has its snapshot").toHaveLength(1);
		writeFileSync(`${f.log}.release`, "");
		expect(await exited).toBe(code);
		expect(snapshots(f)).toEqual([]);
	});
});
