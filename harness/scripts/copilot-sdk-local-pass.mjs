// F250: a local, untracked record that the installed-SDK contract passed on one
// exact Copilot CLI version on this machine.
//
// Copilot updates itself every day or two. The committed
// harness/copilot-sdk-verified.json stays the floor CI and every checkout share,
// but a machine that ran the contract on its own newer CLI needs no PR to say
// so: each full pass writes `<state-dir>/copilot-sdk-pass/<version>.json`, and
// the contract's verdict accepts the installed version when either names it.
// The state dir is pij-rs's own: PIJ_RS_STATE_DIR, else ~/.pij-rs.
//
// A record counts only if it is whole and names exactly the version asked for,
// with the check count the contract has now. Anything else (missing, corrupt,
// partial, another version, a symlink) is treated as absent, so the sensor
// falls back to its warning. Reading never launches a process.
import { constants } from "node:fs";
import { mkdir, open, readFile, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";

/** A CLI version as `copilot --version` prints it; never a path. */
const VERSION = /^[0-9A-Za-z][0-9A-Za-z.+_-]{0,63}$/;
const SHA = /^[0-9a-f]{40}([0-9a-f]{24})?$/;
const MAX_RECORD_BYTES = 4096;

/**
 * Where local passes live: pij-rs's state dir, as `default_state_dir` picks
 * it: `$HOME/.pij-rs`, or with no HOME `<tmpdir>/pij-rs-no-home/.pij-rs`
 * (S307-N1: the fallback is a stand-in HOME, `.pij-rs` still under it).
 */
export function passDir(env = process.env) {
	const state =
		env.PIJ_RS_STATE_DIR || join(env.HOME || join(tmpdir(), "pij-rs-no-home"), ".pij-rs");
	return join(state, "copilot-sdk-pass");
}

function recordPath(dir, version) {
	return VERSION.test(version) ? join(dir, `${version}.json`) : undefined;
}

/**
 * The local pass for exactly `version`, or undefined when there is none or it
 * is not whole: other version, other check count, missing or mistyped field.
 * @param {string} dir
 * @param {string} version
 * @param {number} checkCount the number of checks the contract runs now
 * @returns {Promise<{ version: string, passed_at: string, checks: number, contract_commit: string | null } | undefined>}
 */
export async function readLocalPass(dir, version, checkCount) {
	const path = recordPath(dir, version);
	if (!path) return undefined;
	let text;
	try {
		const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
		try {
			if (!(await file.stat()).isFile()) return undefined;
			const buffer = Buffer.alloc(MAX_RECORD_BYTES + 1);
			const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
			if (bytesRead > MAX_RECORD_BYTES) return undefined;
			text = buffer.subarray(0, bytesRead).toString("utf8");
		} finally {
			await file.close();
		}
	} catch {
		return undefined;
	}
	let record;
	try {
		record = JSON.parse(text);
	} catch {
		return undefined;
	}
	if (record === null || typeof record !== "object") return undefined;
	const { passed_at, checks, contract_commit } = record;
	if (record.version !== version) return undefined;
	if (typeof passed_at !== "string" || Number.isNaN(Date.parse(passed_at))) return undefined;
	if (!Number.isInteger(checks) || checks < 1 || checks !== checkCount) return undefined;
	if (
		contract_commit !== null &&
		!(typeof contract_commit === "string" && SHA.test(contract_commit))
	)
		return undefined;
	return { version, passed_at, checks, contract_commit };
}

/**
 * Record a full pass on `version`: written whole to a new temp file in the same
 * directory, then renamed over `<version>.json`, so a reader sees the old record
 * or the new one, never part of one.
 * @param {string} dir
 * @param {{ version: string, passedAt: Date, checks: number, contractCommit?: string }} pass
 * @returns {Promise<string>} the record's path
 */
export async function writeLocalPass(dir, { version, passedAt, checks, contractCommit }) {
	const path = recordPath(dir, version);
	if (!path) throw new Error(`not a Copilot version: ${JSON.stringify(version)}`);
	await mkdir(dir, { recursive: true, mode: 0o700 });
	const staging = `${path}.${process.pid}.${Date.now()}.tmp`;
	const body = `${JSON.stringify(
		{
			version,
			passed_at: passedAt.toISOString(),
			checks,
			contract_commit: contractCommit ?? null,
		},
		null,
		"\t",
	)}\n`;
	try {
		const file = await open(
			staging,
			constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW,
			0o600,
		);
		try {
			await file.writeFile(body);
			await file.sync();
		} finally {
			await file.close();
		}
		await rename(staging, path);
	} catch (error) {
		await rm(staging, { force: true });
		throw error;
	}
	return path;
}

/**
 * The commit `repo` has checked out, read from its git files in-process (no git
 * launch), or undefined when it can't be read. Handles a linked worktree's
 * `.git` file, a detached HEAD, loose refs and packed refs.
 * @param {string} repo
 * @returns {Promise<string | undefined>}
 */
export async function contractCommit(repo) {
	const read = (path) => readFile(path, "utf8").then((text) => text.trim());
	try {
		const dotGit = join(repo, ".git");
		let gitDir = dotGit;
		const pointer = await read(dotGit).catch(() => undefined);
		if (pointer?.startsWith("gitdir: ")) {
			const target = pointer.slice("gitdir: ".length);
			gitDir = isAbsolute(target) ? target : resolve(repo, target);
		}
		const head = await read(join(gitDir, "HEAD"));
		if (!head.startsWith("ref: ")) return SHA.test(head) ? head : undefined;
		const ref = head.slice("ref: ".length);
		const common = await read(join(gitDir, "commondir"))
			.then((dir) => (isAbsolute(dir) ? dir : resolve(gitDir, dir)))
			.catch(() => gitDir);
		for (const dir of [gitDir, common]) {
			const loose = await read(join(dir, ref)).catch(() => undefined);
			if (loose && SHA.test(loose)) return loose;
		}
		const packed = await read(join(common, "packed-refs")).catch(() => "");
		for (const line of packed.split("\n")) {
			const [sha, name] = line.split(" ");
			if (name === ref && SHA.test(sha)) return sha;
		}
		return undefined;
	} catch {
		return undefined;
	}
}
