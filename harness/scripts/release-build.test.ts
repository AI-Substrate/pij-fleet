import { spawnSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
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
	repo: string;
	home: string;
	built: string;
	log: string;
	commit: string;
	env: NodeJS.ProcessEnv;
}

function fixture(): Fixture {
	const root = realpathSync(mkdtempSync(join(tmpdir(), "pij-release-build-")));
	roots.push(root);
	const repo = join(root, "repo");
	for (const file of [
		".copilot/extensions/pij/extension.mjs",
		".omp/extensions/pij/index.ts",
		"crates/core/tests/fixtures/control-commands.json",
		"crates/core/src/lib.rs",
	]) {
		mkdirSync(join(repo, file, ".."), { recursive: true });
		writeFileSync(join(repo, file), `${file}\n`);
	}
	const git = (...args: string[]) =>
		spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@t", ...args], {
			cwd: repo,
			encoding: "utf8",
		});
	git("init", "-q");
	git("add", ".");
	git("commit", "-q", "-m", "fixture");
	const commit = git("rev-parse", "HEAD").stdout.trim().slice(0, 10);
	const bin = join(root, "bin");
	mkdirSync(bin);
	const target = join(root, "target");
	const built = join(target, "release", "pij-rs");
	mkdirSync(join(target, "release"), { recursive: true });
	// A binary another tree built earlier, unstamped: what the F226 bounce installed.
	binary(built, "unstamped");
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
	mkdirSync(home);
	return {
		repo,
		home,
		built,
		log,
		commit,
		env: {
			...process.env,
			PATH: `${bin}:${process.env.PATH}`,
			CARGO_TARGET_DIR: target,
			PIJ_SHARED_TARGET: undefined,
			PIJ_SHARED_BUILD_DF: df,
			PIJ_ID: "pij-test-seat",
			PIJ_BUILD_COMMIT: undefined,
			LOG: log,
			HOME: home,
		},
	};
}

/** The deploy recipes' own composition: build, then install what it named. */
function deploy(f: Fixture, mode: string) {
	return spawnSync(
		"/bin/sh",
		["-c", 'set -eu; built="$(bash "$1")"; sh "$2" "$built"', "deploy", SCRIPT, INSTALL],
		{ cwd: f.repo, env: { ...f.env, FAKE_CARGO: mode }, encoding: "utf8" },
	);
}

const installed = (f: Fixture) => join(f.home, ".local/share/pij/bin/pij-rs");

describe("release-build.sh, the one deploy build (F226)", () => {
	it("builds through shared-build, stamped, and hands on only this commit's binary", () => {
		const f = fixture();
		const run = spawnSync("bash", [SCRIPT], {
			cwd: f.repo,
			env: { ...f.env, FAKE_CARGO: "ok" },
			encoding: "utf8",
		});
		expect(run.status, run.stderr).toBe(0);
		expect(run.stdout).toBe(`${f.built}\n`);
		const log = readFileSync(f.log, "utf8");
		expect(log).toContain(
			`cargo build --release --locked -p pij-cli --bin pij-rs target=${resolve(f.built, "../..")} incremental=0 commit=${f.commit} copilot=`,
		);
		expect(log).toMatch(/omp=\w{10}\+\w{10}/);
		expect(log).toContain("locked");
		const done = deploy(f, "ok");
		expect(done.status, done.stderr).toBe(0);
		expect(spawnSync(installed(f), ["--version"], { encoding: "utf8" }).stdout).toBe(
			`pij-rs 0.1.0 (${f.commit})\n`,
		);
	});

	it("a failed build exits with cargo's status and installs nothing", () => {
		const f = fixture();
		const run = spawnSync("bash", [SCRIPT], {
			cwd: f.repo,
			env: { ...f.env, FAKE_CARGO: "fail" },
			encoding: "utf8",
		});
		expect(run.status).toBe(101);
		expect(run.stdout).toBe("");
		const done = deploy(f, "fail");
		expect(done.status).not.toBe(0);
		expect(existsSync(installed(f)), "the stale binary must never be installed").toBe(false);
	});

	it("refuses a binary stamped with another commit, and installs nothing", () => {
		const f = fixture();
		const run = spawnSync("bash", [SCRIPT], {
			cwd: f.repo,
			env: { ...f.env, FAKE_CARGO: "foreign" },
			encoding: "utf8",
		});
		expect(run.status).toBe(1);
		expect(run.stdout).toBe("");
		expect(run.stderr).toContain(`not this checkout's commit ${f.commit}`);
		const done = deploy(f, "foreign");
		expect(done.status).not.toBe(0);
		expect(existsSync(installed(f)), "a foreign binary must never be installed").toBe(false);
	});
});
