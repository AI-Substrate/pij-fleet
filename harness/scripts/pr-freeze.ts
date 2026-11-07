// `just pr-freeze <pr> [--worktree]`: is this PR head final for verdicts? (plan 220, F92)
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
import { existsSync, realpathSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** One finished command. */
export interface Ran {
	readonly code: number;
	readonly stdout: string;
	readonly stderr: string;
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
		if (runs.length === 1) {
			state = checkState(runs[0] as Rollup);
		} else if (ids.every((id) => id !== undefined)) {
			const newest = runs.reduce((best, run, index) =>
				(ids[index] ?? 0) > (jobId(best) ?? 0) ? run : best,
			);
			state = checkState(newest);
		} else {
			state = runs.some((run) => checkState(run) === "running") ? "running" : "unordered";
		}
		latest.set(key, { state, runs: runs.length });
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
		ok: !view.isDraft,
		detail: view.isDraft ? "draft" : "ready",
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
		const ci = [...checks.entries()].filter(
			([name]) => name === "delivers" || name.endsWith(" / delivers"),
		);
		const passed = ci.length > 0 && ci.every(([, check]) => check.state === "pass");
		return {
			name: "delivers",
			ok: passed,
			detail: passed
				? "not parsed here (no pij-delivers-check); CI's delivers check passed on this head"
				: `not parsed here (no pij-delivers-check), and CI's delivers check ${ci.length === 0 ? "is absent" : `is ${ci.map(([, check]) => check.state).join(", ")}`}`,
			next: "Next: build the parser (just shared-build -- build -p pij-testkit --bin pij-delivers-check) or wait for CI's delivers check, then rerun",
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

const spawnRunner: Runner = (program, args, input) => {
	const result = spawnSync(program, args, { encoding: "utf8", input, maxBuffer: 64 * 1024 * 1024 });
	return {
		code: result.status ?? (result.error ? 127 : 1),
		stdout: result.stdout ?? "",
		stderr: result.stderr ?? (result.error ? String(result.error) : ""),
	};
};

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

function main(argv: readonly string[]): number {
	const worktree = argv.includes("--worktree");
	const rest = argv.filter((arg) => arg !== "--worktree");
	const pr = Number(rest[0]);
	if (rest.length !== 1 || !Number.isInteger(pr) || pr <= 0) {
		console.error("usage: just pr-freeze <pr number> [--worktree]");
		return 2;
	}
	try {
		const root = must(spawnRunner, "git", ["rev-parse", "--show-toplevel"]).trim();
		const env: Environment = { root, deliversCheck: findDeliversCheck(root) };
		const report = check(pr, spawnRunner, env);
		console.log(render(report));
		if (worktree) {
			console.log(`review worktree: ${createWorktree(report, spawnRunner, env)}`);
		}
		return report.final ? 0 : 1;
	} catch (error) {
		console.error(`pr-freeze: ${error instanceof Error ? error.message : String(error)}`);
		return 2;
	}
}

/** Is this module the script being run? Compared by real path, so a run
 *  through a symlink (macOS /tmp is one) still runs, and never passes silently. */
export function isMain(moduleUrl: string, argv1: string | undefined): boolean {
	if (!argv1) return false;
	try {
		return realpathSync(fileURLToPath(moduleUrl)) === realpathSync(argv1);
	} catch {
		return false;
	}
}

// Run as CLI only (not when imported by tests).
if (isMain(import.meta.url, process.argv[1])) {
	process.exit(main(process.argv.slice(2)));
}
