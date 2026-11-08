// `just pr-freeze <pr> [--worktree]`: is this PR head final for verdicts? (plan 220, F92)
// `just pr-freeze --prune [--dry-run]`: remove review worktrees whose work is over (F107).
//
// Run by hand before each verdict request and before READY. Read-only towards
// the PR: one `git ls-remote`, one `gh pr view`, one `git fetch` of the two
// commits it names (into FETCH_HEAD only, so no ref moves under another seat),
// and local git reads. No loop, timer or poll.
//
// FINAL (exit 0) only when every one of these holds; otherwise exit 1, naming
// each failing condition with its next step:
//   - the head GitHub serves (`ls-remote refs/pull/<n>/head`) and the head gh
//     reports agree;
//   - the base is main;
//   - mergeStateStatus is CLEAN;
//   - every check's LATEST run, by name, passed (a superseded failure doesn't
//     block; an in-progress or queued run does). Runs are ordered by their
//     GitHub Actions job id (detailsUrl); same-name runs that can't be ordered
//     that way are never called final;
//   - the PR is ready, not draft;
//   - every migration the PR adds is numbered above main's highest, and the
//     head's SCHEMA_VERSION is its highest migration (the merge-order rule);
//   - the body's Delivers: line parses with CI's parser (pij-delivers-check),
//     when that binary is built; without it, CI's `delivers` check must be
//     present with its latest run passed.
// It also reports whether a review worktree sits at exactly that head,
// detached and clean; with --worktree it creates one (detached, at the
// ls-remote head) when the two heads agree, and refuses a path that is taken
// by anything else. Exit 2: bad usage or a tool failure. It always prints.

import { spawnSync } from "node:child_process";
import {
	accessSync,
	constants,
	existsSync,
	readdirSync,
	readlinkSync,
	realpathSync,
	statSync,
} from "node:fs";
import { basename, delimiter, dirname, isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

/** One finished command. */
export interface Ran {
	readonly code: number;
	readonly stdout: string;
	readonly stderr: string;
	/** Killed at the child timeout ([`CHILD_TIMEOUT_MS`]). */
	readonly timedOut?: boolean;
}

/** Runs a program with arguments (never a shell) and optional stdin. */
export type Runner = (program: string, args: readonly string[], input?: string) => Ran;

export const MIGRATIONS_DIR = "crates/store/migrations";
export const MIGRATE_RS = "crates/store/src/migrate.rs";

/** Things a check needs that are not git or gh. */
export interface Environment {
	/** The repository's top level. */
	readonly root: string;
	/** The pij-delivers-check binary, when one is built. */
	readonly deliversCheck?: string;
}

export interface Condition {
	readonly name: string;
	readonly ok: boolean;
	/** What was found. */
	readonly detail: string;
	/** What to do when not ok. */
	readonly next?: string;
}

export interface Report {
	readonly pr: number;
	readonly head?: string;
	readonly conditions: readonly Condition[];
	/** Informational lines (never decide FINAL). */
	readonly notes: readonly string[];
	readonly final: boolean;
}

interface Rollup {
	readonly __typename?: string;
	readonly name?: string;
	readonly context?: string;
	readonly workflowName?: string;
	readonly status?: string;
	readonly conclusion?: string;
	readonly state?: string;
	readonly startedAt?: string | null;
	readonly completedAt?: string | null;
	readonly detailsUrl?: string | null;
	readonly targetUrl?: string | null;
}

export interface PrView {
	readonly headRefOid: string;
	readonly baseRefName: string;
	readonly mergeStateStatus: string;
	readonly isDraft: boolean;
	readonly body: string;
	readonly statusCheckRollup: readonly Rollup[];
}

const GH_FIELDS = "headRefOid,baseRefName,mergeStateStatus,isDraft,body,statusCheckRollup";

/** Why each mergeStateStatus is not CLEAN, and what to do about it. */
const MERGE_STATES: Readonly<Record<string, string>> = {
	DIRTY: "conflicts with main. Next: merge origin/main into the branch, resolve, push, and rerun",
	BEHIND: "the branch is behind main. Next: merge origin/main and push",
	UNSTABLE: "a check is failing or still running (see checks). Next: fix or wait, then rerun",
	BLOCKED: "a required review or check is missing. Next: see the PR's merge box",
	DRAFT: "the PR is a draft. Next: gh pr ready <pr>",
	HAS_HOOKS: "mergeable only through pre-receive hooks. Next: ask the prime",
	UNKNOWN: "GitHub is still computing it. Next: rerun in a moment",
};

function short(sha: string | undefined): string {
	return sha ? sha.slice(0, 7) : "?";
}

function must(run: Runner, program: string, args: readonly string[], input?: string): string {
	const result = run(program, args, input);
	if (result.code !== 0) {
		throw new Error(
			`${program} ${args.join(" ")} failed (${result.code}): ${result.stderr.trim()}`,
		);
	}
	return result.stdout;
}

/** `git ls-remote` lines as ref → sha. */
export function parseLsRemote(stdout: string): Map<string, string> {
	const refs = new Map<string, string>();
	for (const line of stdout.split("\n")) {
		const [sha, ref] = line.trim().split(/\s+/);
		if (sha && ref) refs.set(ref, sha);
	}
	return refs;
}

/** A check's state: `unordered` means same-name runs that can't be ordered. */
export type CheckState = "pass" | "running" | "fail" | "unordered";

/** The GitHub Actions job id in a run's URL: ids only grow, so a re-run's is larger. */
export function jobId(check: Rollup): number | undefined {
	const url = check.detailsUrl ?? check.targetUrl ?? "";
	const match = /\/actions\/runs\/\d+\/job\/(\d+)/.exec(url);
	return match?.[1] ? Number(match[1]) : undefined;
}

/** One check name's runs, decided: the latest run's state, or `unordered`. */
export interface LatestCheck {
	readonly state: CheckState;
	readonly runs: number;
	/** The latest run's own conclusion (a status's state), when it is ordered:
	 *  exactly `SUCCESS` is positive evidence; NEUTRAL or SKIPPED pass the
	 *  generic check but prove nothing about what the check verifies. */
	readonly conclusion?: string;
}

/** Each check's latest run, by name. Runs are ordered by job id only (never
 *  by timestamps, which GitHub leaves null on queued runs); when a name has
 *  several runs and any lacks a job id, supersession can't be established, so
 *  the check is `running` if any of its runs isn't finished, else `unordered`. */
export function latestChecks(rollup: readonly Rollup[]): Map<string, LatestCheck> {
	const groups = new Map<string, Rollup[]>();
	for (const check of rollup) {
		const name = check.name ?? check.context ?? "?";
		const key = check.workflowName ? `${check.workflowName} / ${name}` : name;
		groups.set(key, [...(groups.get(key) ?? []), check]);
	}
	const latest = new Map<string, LatestCheck>();
	for (const [key, runs] of groups) {
		const ids = runs.map(jobId);
		let state: CheckState;
		let newest: Rollup | undefined;
		if (runs.length === 1) {
			newest = runs[0];
		} else if (ids.every((id) => id !== undefined)) {
			newest = runs.reduce((best, run, index) =>
				(ids[index] ?? 0) > (jobId(best) ?? 0) ? run : best,
			);
		}
		if (newest) {
			state = checkState(newest);
		} else {
			state = runs.some((run) => checkState(run) === "running") ? "running" : "unordered";
		}
		latest.set(key, {
			state,
			runs: runs.length,
			conclusion: newest ? (newest.conclusion ?? newest.state ?? undefined) : undefined,
		});
	}
	return latest;
}

/** `pass`, `running` or `fail`. */
export function checkState(check: Rollup): "pass" | "running" | "fail" {
	if (check.__typename === "StatusContext" || (check.state && !check.status)) {
		if (check.state === "SUCCESS") return "pass";
		return check.state === "PENDING" || check.state === "EXPECTED" ? "running" : "fail";
	}
	if (check.status !== "COMPLETED") return "running";
	return ["SUCCESS", "NEUTRAL", "SKIPPED"].includes(check.conclusion ?? "") ? "pass" : "fail";
}

/** The migration numbers in a `git ls-tree --name-only` or `git diff --name-only` listing. */
export function migrationNumbers(listing: string): number[] {
	const numbers = new Set<number>();
	for (const line of listing.split("\n")) {
		const match = /(?:^|\/)(\d{4})_[^/]*\.sql$/.exec(line.trim());
		if (match?.[1]) numbers.add(Number(match[1]));
	}
	return [...numbers].sort((a, b) => a - b);
}

export function schemaVersion(source: string): number | undefined {
	const match = /pub const SCHEMA_VERSION: u32 = (\d+);/.exec(source);
	return match?.[1] ? Number(match[1]) : undefined;
}

interface WorktreeEntry {
	readonly path: string;
	readonly head?: string;
	readonly detached: boolean;
}

function worktrees(porcelain: string): WorktreeEntry[] {
	return porcelain
		.split("\n\n")
		.map((block) => ({
			path: /^worktree (.+)$/m.exec(block)?.[1] ?? "",
			head: /^HEAD ([0-9a-f]+)$/m.exec(block)?.[1],
			detached: /^detached$/m.test(block),
		}))
		.filter((entry) => entry.path !== "");
}

/** What `git worktree list` says about this PR's review worktree path(s). */
export interface ReviewWorktree {
	/** A worktree detached at exactly the head, named `-<pr>-<head7>`, and clean. */
	readonly usable?: string;
	/** Paths with that name that are NOT usable, and why. */
	readonly unusable: readonly string[];
}

/** The review worktree for this head: detached at exactly the head, named
 *  `-<pr>-<head7>`, with nothing modified or untracked. Anything else under
 *  that name is reported as unusable, never reused. */
export function reviewWorktree(
	porcelain: string,
	pr: number,
	head: string,
	run: Runner,
): ReviewWorktree {
	const unusable: string[] = [];
	for (const entry of worktrees(porcelain)) {
		if (!basename(entry.path).endsWith(`-${pr}-${short(head)}`)) continue;
		if (!entry.detached) {
			unusable.push(`${entry.path} is on a branch, not detached`);
		} else if (entry.head !== head) {
			unusable.push(`${entry.path} is at ${short(entry.head)}, not ${short(head)}`);
		} else {
			const status = run("git", ["-C", entry.path, "status", "--porcelain"]);
			if (status.code !== 0) {
				unusable.push(`${entry.path} can't be read (git status failed)`);
			} else if (status.stdout.trim() !== "") {
				unusable.push(`${entry.path} has local changes`);
			} else {
				return { usable: entry.path, unusable };
			}
		}
	}
	return { unusable };
}

/** Every condition for `pr`, from git and gh. Throws on a tool failure. */
export function check(pr: number, run: Runner, env: Environment): Report {
	const pull = `refs/pull/${pr}/head`;
	const remote = parseLsRemote(must(run, "git", ["ls-remote", "origin", "refs/heads/main", pull]));
	const served = remote.get(pull);
	const mainSha = remote.get("refs/heads/main");
	const view = JSON.parse(
		must(run, "gh", ["pr", "view", String(pr), "--json", GH_FIELDS]),
	) as PrView;
	const conditions: Condition[] = [];
	const notes: string[] = [];

	const agree = served !== undefined && served === view.headRefOid;
	conditions.push({
		name: "head",
		ok: agree,
		detail: `${served ?? "(none served)"} (ls-remote), ${view.headRefOid} (gh)`,
		next: agree
			? undefined
			: "the two disagree: GitHub hasn't caught up, or the branch moved. Next: rerun in a moment; if it still differs, the head is moving, so don't request verdicts",
	});
	conditions.push({
		name: "base",
		ok: view.baseRefName === "main",
		detail: view.baseRefName,
		next: `Next: gh pr edit ${pr} --base main`,
	});
	const merge = view.mergeStateStatus;
	conditions.push({
		name: "merge",
		ok: merge === "CLEAN",
		detail: merge === "CLEAN" ? "CLEAN" : `${merge}: ${MERGE_STATES[merge] ?? "not CLEAN"}`,
	});

	const latest = latestChecks(view.statusCheckRollup ?? []);
	const checks = [...latest.entries()];
	const named = (state: CheckState) =>
		checks.filter(([, check]) => check.state === state).map(([name]) => name);
	const [failing, running, unordered] = [named("fail"), named("running"), named("unordered")];
	conditions.push({
		name: "checks",
		ok: checks.length > 0 && checks.every(([, check]) => check.state === "pass"),
		detail:
			checks.length === 0
				? "no checks ran on this head"
				: checks.map(([name, check]) => `${name} ${check.state}`).join(", "),
		next:
			checks.length === 0
				? "Next: CI hasn't started (or the PR is DIRTY); push or wait, then rerun"
				: failing.length > 0
					? `Next: fix ${failing.join(", ")} and push`
					: running.length > 0
						? `Next: wait for ${running.join(", ")}, then rerun`
						: `Next: ${unordered.join(", ")} ran more than once and the runs can't be ordered; re-run it in CI, then rerun`,
	});
	conditions.push({
		name: "draft",
		// Positive evidence only: an absent isDraft is not "ready".
		ok: view.isDraft === false,
		detail:
			view.isDraft === false
				? "ready"
				: view.isDraft === true
					? "draft"
					: "draft state not reported",
		next: `Next: gh pr ready ${pr}`,
	});

	if (served && mainSha) {
		must(run, "git", ["fetch", "-q", "--no-tags", "origin", mainSha, served]);
		conditions.push(migrations(run, mainSha, served));
	} else {
		conditions.push({
			name: "migrations",
			ok: false,
			detail: "main or the PR head is not served by origin",
			next: "Next: check the PR exists and origin is the PR's repository",
		});
	}
	conditions.push(delivers(run, view.body ?? "", env, latest));

	if (served) {
		const porcelain = must(run, "git", ["worktree", "list", "--porcelain"]);
		const found = reviewWorktree(porcelain, pr, served, run);
		for (const why of found.unusable) notes.push(`worktree  not usable: ${why}`);
		notes.push(
			found.usable
				? `worktree  ${found.usable} (detached at ${short(served)}, clean)`
				: found.unusable.length > 0
					? `worktree  none usable at -${pr}-${short(served)}. Remove or rename the one above, then: just pr-freeze ${pr} --worktree`
					: `worktree  none at -${pr}-${short(served)}. Create one: just pr-freeze ${pr} --worktree`,
		);
	}
	return {
		pr,
		head: agree ? served : undefined,
		conditions,
		notes,
		final: conditions.every((condition) => condition.ok),
	};
}

/** The migrations the PR adds against main's, and SCHEMA_VERSION on both sides. */
function migrations(run: Runner, mainSha: string, head: string): Condition {
	const onMain = migrationNumbers(
		must(run, "git", ["ls-tree", "--name-only", mainSha, `${MIGRATIONS_DIR}/`]),
	);
	const added = migrationNumbers(
		must(run, "git", [
			"diff",
			"--name-only",
			"--diff-filter=A",
			`${mainSha}...${head}`,
			"--",
			MIGRATIONS_DIR,
		]),
	);
	const highest = onMain.at(-1) ?? 0;
	const mainSchema = schemaVersion(must(run, "git", ["show", `${mainSha}:${MIGRATE_RS}`]));
	const headSchema = schemaVersion(must(run, "git", ["show", `${head}:${MIGRATE_RS}`]));
	const versions = `SCHEMA_VERSION main ${mainSchema ?? "?"}, head ${headSchema ?? "?"}`;
	if (added.length === 0) {
		return {
			name: "migrations",
			ok: true,
			detail: `none added; main's highest ${highest}; ${versions}`,
		};
	}
	const low = added.filter((number) => number <= highest);
	const top = added.at(-1) ?? 0;
	const problems: string[] = [];
	if (low.length > 0) problems.push(`${low.join(", ")} not above main's highest ${highest}`);
	if (headSchema !== top) problems.push(`head SCHEMA_VERSION ${headSchema ?? "?"} is not ${top}`);
	return {
		name: "migrations",
		ok: problems.length === 0,
		detail: `adds ${added.join(", ")}; main's highest ${highest}; ${versions}${problems.length ? `: ${problems.join("; ")}` : ""}`,
		next: `Next: renumber above ${highest} (ask pm-001 for the number), set SCHEMA_VERSION to match, and push`,
	};
}

/** The body's Delivers: line, through the parser CI runs; without the
 *  binary, only a present and passed CI `delivers` check stands in. */
function delivers(
	run: Runner,
	body: string,
	env: Environment,
	checks: Map<string, LatestCheck>,
): Condition {
	if (!env.deliversCheck) {
		// Exactly SUCCESS: a NEUTRAL or SKIPPED delivers run parsed nothing.
		const ci = [...checks.entries()].filter(
			([name]) => name === "delivers" || name.endsWith(" / delivers"),
		);
		const succeeded = ci.length > 0 && ci.every(([, check]) => check.conclusion === "SUCCESS");
		const seen = ci.map(([, check]) =>
			check.state === "pass" ? (check.conclusion ?? "pass") : check.state,
		);
		return {
			name: "delivers",
			ok: succeeded,
			detail: succeeded
				? "not parsed here (no pij-delivers-check); CI's delivers check succeeded on this head"
				: `not parsed here (no pij-delivers-check), and CI's delivers check ${ci.length === 0 ? "is absent" : `is ${seen.join(", ")}, not SUCCESS`}`,
			next: "Next: build the parser (just shared-build -- build -p pij-testkit --bin pij-delivers-check) or get CI's delivers check to run and succeed, then rerun",
		};
	}
	const result = run(env.deliversCheck, [], body);
	const said = (result.code === 0 ? result.stdout : result.stderr).trim().split("\n")[0] ?? "";
	return {
		name: "delivers",
		ok: result.code === 0,
		detail: said,
		next: "Next: put `Delivers: none` or full ddocs addresses on one line in the PR body",
	};
}

/** Create the review worktree at the verified head, beside the repository. */
export function createWorktree(report: Report, run: Runner, env: Environment): string {
	if (!report.head) {
		throw new Error(
			"refusing --worktree: ls-remote and gh disagree about the head (see head above)",
		);
	}
	const porcelain = must(run, "git", ["worktree", "list", "--porcelain"]);
	const found = reviewWorktree(porcelain, report.pr, report.head, run);
	if (found.usable) return found.usable;
	const path = join(dirname(env.root), `review-${report.pr}-${short(report.head)}`);
	const taken = worktrees(porcelain).some((entry) => entry.path === path) || existsSync(path);
	if (taken || found.unusable.length > 0) {
		throw new Error(
			`refusing --worktree: ${[...found.unusable, ...(taken ? [`${path} already exists`] : [])].join("; ")}. Remove it (git worktree remove <path>), then rerun`,
		);
	}
	must(run, "git", ["worktree", "add", "--detach", path, report.head]);
	return path;
}

// `--prune` (F107): remove the review worktrees `--worktree` made once their
// work is over. One input DECIDES removal, the PR's state on GitHub:
//   - the PR is MERGED or CLOSED, or
//   - it is OPEN and its head has moved on from the worktree's (superseded).
// Everything else is a BRAKE, which can only spare a worktree, never remove
// one (remove any of them and prune removes the same set or more):
//   - the worktree has uncommitted or untracked changes, or git can't say;
//   - a live seat's folder (`pij-rs list`) or any process's cwd is in it;
//   - it isn't one `--worktree` made: not named review-<pr>-<sha7>, not
//     detached, or not at that sha;
//   - gh can't read its PR.
// If the seat list or the process cwds can't be read, nothing is removed.
// Removal is `git worktree remove` without --force, which refuses a dirty
// worktree on its own too. On demand only: one gh read per PR, one git status
// per candidate, one seat list and one cwd listing per run.

/** A worktree `--worktree` may have made: `review-<pr>-<sha7>`. */
export interface PruneCandidate {
	readonly path: string;
	readonly pr: number;
	readonly sha7: string;
	readonly head?: string;
	readonly detached: boolean;
}

/** The PR as GitHub reports it. */
export interface PrState {
	readonly state: string;
	readonly headRefOid: string;
}

/** What prune does with one worktree, and why. */
export interface PruneVerdict {
	readonly path: string;
	readonly remove: boolean;
	readonly why: string;
}

/** Every worktree named `review-<pr>-<sha7>`. */
export function pruneCandidates(porcelain: string): PruneCandidate[] {
	const found: PruneCandidate[] = [];
	for (const entry of worktrees(porcelain)) {
		const match = /^review-(\d+)-([0-9a-f]{7})$/.exec(basename(entry.path));
		if (!match?.[1] || !match[2]) continue;
		found.push({ ...entry, pr: Number(match[1]), sha7: match[2] });
	}
	return found;
}

/** Whether `path` is `root` or inside it. */
export function within(path: string, root: string): boolean {
	const trimmed = root.replace(/\/+$/, "");
	return path === trimmed || path.startsWith(`${trimmed}/`);
}

/** Decide one candidate: the PR's state decides; `dirty` and `busy` (each a
 *  reason, when one applies) only ever spare it. */
export function decide(
	candidate: PruneCandidate,
	pr: PrState | undefined,
	dirty: string | undefined,
	busy: string | undefined,
): PruneVerdict {
	const keep = (why: string): PruneVerdict => ({ path: candidate.path, remove: false, why });
	if (!candidate.detached) return keep("on a branch, so not one --worktree made");
	if (!candidate.head?.startsWith(candidate.sha7)) {
		return keep(`at ${short(candidate.head)}, not the ${candidate.sha7} its name says`);
	}
	if (!pr) return keep(`PR #${candidate.pr}'s state can't be read`);
	let over: string;
	if (pr.state === "MERGED" || pr.state === "CLOSED") {
		over = `PR #${candidate.pr} is ${pr.state.toLowerCase()}`;
	} else if (pr.headRefOid !== candidate.head) {
		over = `PR #${candidate.pr}'s head moved on to ${short(pr.headRefOid)}`;
	} else {
		return keep(`PR #${candidate.pr} is open at this head`);
	}
	if (dirty) return keep(`${over}, but ${dirty}`);
	if (busy) return keep(`${over}, but ${busy}`);
	return { path: candidate.path, remove: true, why: over };
}

/** The folders of every live seat, from `pij-rs --json list`. */
function seatFolders(run: Runner): { id: string; folder: string }[] {
	const listed = JSON.parse(must(run, "pij-rs", ["--json", "list"])) as {
		data?: { seats?: { id?: string; folder?: string }[] };
	};
	const seats = listed.data?.seats;
	if (!Array.isArray(seats)) throw new Error("pij-rs list returned no seats array");
	return seats
		.filter((seat) => typeof seat.folder === "string" && seat.folder !== "")
		.map((seat) => ({ id: seat.id ?? "?", folder: seat.folder as string }));
}

/** Every process's cwd that this user may inspect: one `lsof -d cwd -F pn`
 *  on macOS, `/proc` on Linux. Unprivileged lsof lists only this user's
 *  processes, so Linux skips other users' pids by owner, the same scope.
 *  Anything unsuccessful, incomplete or malformed throws, so prune removes
 *  nothing (review of #170, S170-001): a nonzero exit (a timeout included),
 *  anything on stderr, or a record that doesn't parse. */
export function processCwds(
	run: Runner,
	platform: string = process.platform,
	readProc: () => string[] = () => linuxCwds(),
): { pid: string; cwd: string }[] {
	if (platform === "linux") {
		return readProc().map((line) => {
			const [pid = "?", cwd = ""] = line.split("\t");
			return { pid, cwd };
		});
	}
	const listed = run("/usr/sbin/lsof", ["-d", "cwd", "-F", "pn"]);
	if (listed.code !== 0 || listed.stderr.trim() !== "") {
		throw new Error(
			`lsof did not list every cwd (exit ${listed.code}${listed.timedOut ? ", timed out" : ""}): ${listed.stderr.trim() || "no message"}`,
		);
	}
	const cwds: { pid: string; cwd: string }[] = [];
	let pid: string | undefined;
	for (const line of listed.stdout.split("\n")) {
		if (line === "") continue;
		if (/^p\d+$/.test(line)) {
			pid = line.slice(1);
		} else if (line.startsWith("f")) {
			// The file descriptor field: cwd, as asked.
		} else if (line.startsWith("n") && line.length > 1 && pid !== undefined) {
			cwds.push({ pid, cwd: line.slice(1) });
		} else {
			throw new Error(`lsof printed a record that doesn't parse: ${JSON.stringify(line)}`);
		}
	}
	if (cwds.length === 0) throw new Error("lsof listed no process cwds");
	return cwds;
}

/** What [`linuxCwds`] reads; injectable for tests. */
export interface ProcFs {
	readonly pids: () => string[];
	readonly owner: (pid: string) => number;
	readonly cwd: (pid: string) => string;
	readonly uid: number;
}

const realProc: ProcFs = {
	pids: () => readdirSync("/proc").filter((name) => /^\d+$/.test(name)),
	owner: (pid) => statSync(`/proc/${pid}`).uid,
	cwd: (pid) => readlinkSync(`/proc/${pid}/cwd`),
	uid: process.getuid?.() ?? -1,
};

/** A process that ended between listing and reading: ENOENT or ESRCH. */
function gone(error: unknown): boolean {
	const code = (error as { code?: string } | undefined)?.code;
	return code === "ENOENT" || code === "ESRCH";
}

/** `<pid>\t<cwd>` for each of this user's processes. A process that ended is
 *  skipped; any other read error throws (S170-001). */
export function linuxCwds(proc: ProcFs = realProc): string[] {
	const lines: string[] = [];
	for (const pid of proc.pids()) {
		try {
			if (proc.owner(pid) !== proc.uid) continue;
			lines.push(`${pid}\t${proc.cwd(pid)}`);
		} catch (error) {
			if (gone(error)) continue;
			throw new Error(
				`can't read /proc/${pid}/cwd (${error instanceof Error ? error.message : String(error)})`,
			);
		}
	}
	return lines;
}

/** The real path, or undefined when it doesn't resolve. */
function real(path: string): string | undefined {
	try {
		return realpathSync(path);
	} catch {
		return undefined;
	}
}

/** `path` and its real path, when that differs: both sides of a comparison
 *  get both forms (S170-001), so a symlinked spelling still matches. */
function spellings(path: string): string[] {
	const resolved = real(path);
	return resolved && resolved !== path ? [path, resolved] : [path];
}

/** Decide every review worktree and, unless `dryRun`, remove those whose work
 *  is over. Everything is read before anything is removed; a seat list, cwd
 *  listing or child that fails or times out throws, removing nothing. */
export function prune(
	run: Runner,
	dryRun: boolean,
	cwds: () => { pid: string; cwd: string }[] = () => processCwds(run),
): PruneVerdict[] {
	const candidates = pruneCandidates(must(run, "git", ["worktree", "list", "--porcelain"]));
	if (candidates.length === 0) return [];
	const refuse = (why: string): never => {
		throw new Error(`refusing --prune: ${why}; nothing was removed`);
	};
	let seats: { id: string; folders: string[] }[] = [];
	let processes: { pid: string; cwds: string[] }[] = [];
	try {
		seats = seatFolders(run).map((seat) => ({ id: seat.id, folders: spellings(seat.folder) }));
		processes = cwds().map((proc) => ({ pid: proc.pid, cwds: spellings(proc.cwd) }));
	} catch (error) {
		refuse(
			`can't tell which worktrees are in use (${error instanceof Error ? error.message : String(error)})`,
		);
	}
	const timedOut = (ran: Ran, what: string) => {
		if (ran.timedOut) refuse(`${what} timed out`);
	};
	const states = new Map<number, PrState | undefined>();
	for (const pr of new Set(candidates.map((candidate) => candidate.pr))) {
		const view = run("gh", ["pr", "view", String(pr), "--json", "state,headRefOid"]);
		timedOut(view, `gh pr view ${pr}`);
		states.set(pr, view.code === 0 ? (JSON.parse(view.stdout) as PrState) : undefined);
	}
	const decided: PruneVerdict[] = [];
	for (const candidate of candidates) {
		const status = run("git", [
			"-C",
			candidate.path,
			"status",
			"--porcelain",
			"--untracked-files=all",
		]);
		timedOut(status, `git status in ${candidate.path}`);
		const dirty =
			status.code !== 0
				? "git status can't read it"
				: status.stdout.trim() !== ""
					? "it has local changes"
					: undefined;
		const resolved = real(candidate.path);
		const paths = resolved ? [candidate.path, resolved] : [];
		const seat = seats.find((seat) =>
			seat.folders.some((folder) => paths.some((path) => within(folder, path))),
		);
		const proc = processes.find((proc) =>
			proc.cwds.some((cwd) => paths.some((path) => within(cwd, path))),
		);
		const busy = !resolved
			? "its path doesn't resolve, so pij can't tell who is in it"
			: seat
				? `seat ${seat.id}'s folder is in it`
				: proc
					? `process ${proc.pid}'s cwd is in it`
					: undefined;
		decided.push(decide(candidate, states.get(candidate.pr), dirty, busy));
	}
	if (dryRun) return decided;
	const verdicts: PruneVerdict[] = [];
	let stopped: string | undefined;
	for (const verdict of decided) {
		if (!verdict.remove) {
			verdicts.push(verdict);
			continue;
		}
		if (stopped) {
			verdicts.push({ ...verdict, remove: false, why: `${verdict.why}, but ${stopped}` });
			continue;
		}
		const removed = run("git", ["worktree", "remove", verdict.path]);
		if (removed.code !== 0) {
			if (removed.timedOut) stopped = "an earlier removal timed out, so prune stopped";
			verdicts.push({
				...verdict,
				remove: false,
				why: `${verdict.why}, but git worktree remove ${removed.timedOut ? "timed out" : `refused: ${removed.stderr.trim()}`}`,
			});
			continue;
		}
		verdicts.push(verdict);
	}
	return verdicts;
}

export function renderPrune(verdicts: readonly PruneVerdict[], dryRun: boolean): string {
	if (verdicts.length === 0) return "pr-freeze --prune: no review-<pr>-<sha7> worktrees";
	const removed = verdicts.filter((verdict) => verdict.remove).length;
	const lines = verdicts.map(
		(verdict) =>
			`  ${verdict.remove ? (dryRun ? "would remove" : "removed") : "kept"} ${verdict.path}: ${verdict.why}`,
	);
	lines.unshift(
		`pr-freeze --prune${dryRun ? " --dry-run" : ""}: ${removed} of ${verdicts.length} ${dryRun ? "would be removed" : "removed"}`,
	);
	return lines.join("\n");
}

export function render(report: Report): string {
	const lines = [`pr-freeze #${report.pr}`];
	for (const condition of report.conditions) {
		lines.push(
			`  ${condition.ok ? "ok  " : "FAIL"} ${condition.name.padEnd(10)} ${condition.detail}`,
		);
	}
	for (const note of report.notes) lines.push(`       ${note}`);
	const failing = report.conditions.filter((condition) => !condition.ok);
	if (failing.length === 0) {
		lines.push(`FINAL: #${report.pr} at ${report.head}`);
	} else {
		lines.push(`NOT FINAL: ${failing.length} condition(s) fail:`);
		for (const condition of failing) {
			lines.push(
				`  - ${condition.name}: ${condition.detail}${condition.next ? `. ${condition.next}` : ""}`,
			);
		}
	}
	return lines.join("\n");
}

/** How long any one child (git, gh, pij-rs, lsof) may run. A timeout is a
 *  failure: prune refuses and removes nothing (review of #170). */
export const CHILD_TIMEOUT_MS = 60_000;

const spawnRunner: Runner = (program, args, input) => {
	const result = spawnSync(program, args, {
		encoding: "utf8",
		input,
		maxBuffer: 64 * 1024 * 1024,
		timeout: CHILD_TIMEOUT_MS,
	});
	const timedOut = (result.error as { code?: string } | undefined)?.code === "ETIMEDOUT";
	return {
		code: result.status ?? (result.error ? 127 : 1),
		stdout: result.stdout ?? "",
		stderr: timedOut
			? `timed out after ${CHILD_TIMEOUT_MS / 1000}s`
			: (result.stderr ?? (result.error ? String(result.error) : "")),
		timedOut,
	};
};

/** Whether `file` is an executable regular file. */
function executable(file: string): boolean {
	try {
		accessSync(file, constants.X_OK);
		return statSync(file).isFile();
	} catch {
		return false;
	}
}

/** Each of `tools` as an absolute path, found once by walking PATH in-process
 *  (no `which`, no launch). Relative PATH entries are skipped. Throws naming
 *  every tool that isn't found (review of #170, P170-F001). */
export function resolveTools(
	tools: readonly string[],
	path: string = process.env.PATH ?? "",
	isExecutable: (file: string) => boolean = executable,
): Map<string, string> {
	const dirs = path.split(delimiter).filter((dir) => dir !== "" && isAbsolute(dir));
	const found = new Map<string, string>();
	const missing: string[] = [];
	for (const tool of tools) {
		const hit = dirs.map((dir) => join(dir, tool)).find(isExecutable);
		if (hit) found.set(tool, hit);
		else missing.push(tool);
	}
	if (missing.length > 0) {
		throw new Error(
			`${missing.join(", ")} not found on PATH; install ${missing.length > 1 ? "them" : "it"} or add ${missing.length > 1 ? "their" : "its"} directory to PATH, then rerun`,
		);
	}
	return found;
}

/** `base`, launching each bare tool name by its resolved absolute path. A
 *  bare name that wasn't resolved is refused, never launched by PATH. */
export function resolvedRunner(tools: ReadonlyMap<string, string>, base: Runner): Runner {
	return (program, args, input) => {
		if (isAbsolute(program)) return base(program, args, input);
		const path = tools.get(program);
		if (!path) {
			return { code: 127, stdout: "", stderr: `${program} was not resolved to an absolute path` };
		}
		return base(path, args, input);
	};
}

/** The built pij-delivers-check: PIJ_DELIVERS_CHECK, the shared target, or this tree's target. */
function findDeliversCheck(root: string): string | undefined {
	if (process.env.PIJ_DELIVERS_CHECK) return process.env.PIJ_DELIVERS_CHECK;
	const targets = [
		process.env.PIJ_SHARED_TARGET,
		process.env.CARGO_TARGET_DIR,
		join(root, "target"),
	];
	for (const target of targets) {
		for (const profile of ["release", "debug"]) {
			const candidate = target ? join(target, profile, "pij-delivers-check") : undefined;
			if (candidate && existsSync(candidate)) return candidate;
		}
	}
	return undefined;
}

const USAGE =
	"usage: just pr-freeze <pr number> [--worktree], or just pr-freeze --prune [--dry-run]";

function main(argv: readonly string[]): number {
	if (argv.includes("--prune")) {
		const dryRun = argv.includes("--dry-run");
		if (argv.some((arg) => arg !== "--prune" && arg !== "--dry-run")) {
			console.error(USAGE);
			return 2;
		}
		try {
			const run = resolvedRunner(resolveTools(["git", "gh", "pij-rs"]), spawnRunner);
			console.log(renderPrune(prune(run, dryRun), dryRun));
			return 0;
		} catch (error) {
			console.error(`pr-freeze: ${error instanceof Error ? error.message : String(error)}`);
			return 2;
		}
	}
	const worktree = argv.includes("--worktree");
	const rest = argv.filter((arg) => arg !== "--worktree");
	const pr = Number(rest[0]);
	if (rest.length !== 1 || !Number.isInteger(pr) || pr <= 0) {
		console.error(USAGE);
		return 2;
	}
	try {
		const run = resolvedRunner(resolveTools(["git", "gh"]), spawnRunner);
		const root = must(run, "git", ["rev-parse", "--show-toplevel"]).trim();
		const env: Environment = { root, deliversCheck: findDeliversCheck(root) };
		const report = check(pr, run, env);
		console.log(render(report));
		if (worktree) {
			console.log(`review worktree: ${createWorktree(report, run, env)}`);
		}
		return report.final ? 0 : 1;
	} catch (error) {
		console.error(`pr-freeze: ${error instanceof Error ? error.message : String(error)}`);
		return 2;
	}
}

/** Is this module the script being run? `run` or `import` only on a proven
 *  comparison of real paths, so a run through a symlink (macOS /tmp is one)
 *  still runs; a lookup that fails is neither (`error`), never a silent skip. */
export type Entry =
	| { readonly kind: "run" }
	| { readonly kind: "import" }
	| { readonly kind: "error"; readonly message: string };

export function entry(
	moduleUrl: string,
	argv1: string | undefined,
	realpath: (path: string) => string = realpathSync,
): Entry {
	// No script path: imported into a REPL or an `-e` program, not run.
	if (!argv1) return { kind: "import" };
	try {
		return realpath(fileURLToPath(moduleUrl)) === realpath(argv1)
			? { kind: "run" }
			: { kind: "import" };
	} catch (error) {
		return {
			kind: "error",
			message: `pr-freeze: could not tell whether it was run as a script (${error instanceof Error ? error.message : String(error)}); refusing to exit silently`,
		};
	}
}

/** Act on [`entry`]: run, do nothing for an import, or print the lookup
 *  failure and exit 2. Never prints nothing and exits 0 as a script. */
export function guard(
	decided: Entry,
	run: () => number,
	exit: (code: number) => void,
	report: (message: string) => void,
): void {
	if (decided.kind === "run") {
		exit(run());
	} else if (decided.kind === "error") {
		report(decided.message);
		exit(2);
	}
}

guard(
	entry(import.meta.url, process.argv[1]),
	() => main(process.argv.slice(2)),
	(code) => process.exit(code),
	(message) => console.error(message),
);
