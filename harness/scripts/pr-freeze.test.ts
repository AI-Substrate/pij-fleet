import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";
import {
	check,
	checkState,
	createWorktree,
	decide,
	entry,
	guard,
	latestChecks,
	MACOS_ONLY,
	migrationNumbers,
	type PruneCandidate,
	processCwds,
	prState,
	prune,
	type Ran,
	type Runner,
	render,
	renderPrune,
	resolvedRunner,
	resolveTools,
} from "./pr-freeze.js";

const SCRIPT = resolve(import.meta.dirname, "pr-freeze.ts");
/** How the `pr-freeze` recipe runs it: node alone, no node_modules (F109). */
const NODE = [
	process.execPath,
	"--experimental-strip-types",
	"--disable-warning=ExperimentalWarning",
];

const PR = 7;
const HEAD = "a7a0f4a470532ae54d0103528b2bf338abff3bfe";
const OTHER = "08f74822302742c63614f87479f1158918718161";
const MAIN = "cee011387937c7881318c98d86014526d94f2ca7";
const ROOT = "/work/fleet";
const PARSER = "/fake/pij-delivers-check";

interface World {
	served: string | undefined;
	ghHead: string;
	base: string;
	merge: string;
	draft: boolean;
	body: string;
	rollup: object[];
	mainMigrations: number[];
	added: number[];
	mainSchema: number;
	headSchema: number;
	worktrees: string;
	dirty: string[];
}

/** A finished, passed Actions check whose job id is `job` (ids only grow). */
const pass = (name: string, job = 100, workflowName = "ci") => ({
	__typename: "CheckRun",
	name,
	workflowName,
	status: "COMPLETED",
	conclusion: "SUCCESS",
	startedAt: "2026-11-07T14:00:00Z",
	detailsUrl: `https://github.com/o/r/actions/runs/7/job/${job}`,
});

function good(): World {
	return {
		served: HEAD,
		ghHead: HEAD,
		base: "main",
		merge: "CLEAN",
		draft: false,
		body: "Delivers: none\n\n## What changed\n",
		rollup: [pass("rust-test"), pass("rust-lint")],
		mainMigrations: [35, 36],
		added: [],
		mainSchema: 36,
		headSchema: 36,
		worktrees: `worktree ${ROOT}\nHEAD ${MAIN}\nbranch refs/heads/main\n`,
		dirty: [],
	};
}

const ok = (stdout = ""): Ran => ({ code: 0, stdout, stderr: "" });
const migration = (n: number) => `crates/store/migrations/${String(n).padStart(4, "0")}_x.sql`;

/** A fake gh and git over `world`; every call is recorded. */
function fake(world: World): { run: Runner; calls: string[] } {
	const calls: string[] = [];
	const run: Runner = (program, args, input) => {
		calls.push([program, ...args].join(" "));
		if (program === PARSER) {
			return /^Delivers: (none|docs\/\S+)\s*$/m.test(input ?? "")
				? ok("Delivers: none\n")
				: { code: 1, stdout: "", stderr: "pij-delivers-check: no parseable Delivers: line\n" };
		}
		if (args[0] === "-C") {
			return ok(world.dirty.includes(args[1] ?? "") ? " M src/lib.rs\n" : "");
		}
		const [verb, ...rest] = args;
		if (program === "gh") {
			return ok(
				JSON.stringify({
					headRefOid: world.ghHead,
					baseRefName: world.base,
					mergeStateStatus: world.merge,
					isDraft: world.draft,
					body: world.body,
					statusCheckRollup: world.rollup,
				}),
			);
		}
		switch (verb) {
			case "ls-remote":
				return ok(
					`${MAIN}\trefs/heads/main\n${world.served ? `${world.served}\trefs/pull/${PR}/head\n` : ""}`,
				);
			case "fetch":
				return ok();
			case "ls-tree":
				return ok(world.mainMigrations.map(migration).join("\n"));
			case "diff":
				return ok(world.added.map(migration).join("\n"));
			case "show":
				return ok(
					`pub const SCHEMA_VERSION: u32 = ${rest[0]?.startsWith(MAIN) ? world.mainSchema : world.headSchema};\n`,
				);
			case "worktree":
				return rest[0] === "list" ? ok(world.worktrees) : ok();
			default:
				return { code: 1, stdout: "", stderr: `unexpected git ${args.join(" ")}` };
		}
	};
	return { run, calls };
}

/** The failing conditions' names; `parser: null` means none is built. */
function failing(world: World, parser: string | null = PARSER): string[] {
	const report = check(PR, fake(world).run, { root: ROOT, deliversCheck: parser ?? undefined });
	return report.conditions.filter((condition) => !condition.ok).map((condition) => condition.name);
}

describe("pr-freeze (plan 220, F92)", () => {
	it("says FINAL, exit 0, only when every condition holds", () => {
		const { run, calls } = fake(good());
		const report = check(PR, run, { root: ROOT, deliversCheck: PARSER });
		expect(report.final).toBe(true);
		expect(report.head).toBe(HEAD);
		expect(render(report)).toContain(`FINAL: #${PR} at ${HEAD}`);
		// Read-only: one ls-remote, one gh view, one fetch of the two shas.
		expect(calls.filter((call) => call.startsWith("git ls-remote"))).toHaveLength(1);
		expect(calls.filter((call) => call.startsWith("gh "))).toHaveLength(1);
		expect(calls).toContain(`git fetch -q --no-tags origin ${MAIN} ${HEAD}`);
		expect(calls.some((call) => call.startsWith("git worktree add"))).toBe(false);
	});

	it.each<[string, (world: World) => void]>([
		["head", (w) => (w.ghHead = OTHER)],
		["head", (w) => (w.served = undefined)],
		["base", (w) => (w.base = "plan/203-prime-prompts")],
		["merge", (w) => (w.merge = "DIRTY")],
		["merge", (w) => (w.merge = "BEHIND")],
		[
			"checks",
			(w) => (w.rollup = [pass("rust-lint"), { ...pass("rust-test"), conclusion: "FAILURE" }]),
		],
		["checks", (w) => (w.rollup = [])],
		["draft", (w) => (w.draft = true)],
		["migrations", (w) => (w.added = [36])],
		["delivers", (w) => (w.body = "Delivers: the s05 send path, roughly\n")],
		["delivers", (w) => (w.body = "## What changed\nno line at all\n")],
	])("fails %s alone, naming it with a next step", (name, spoil) => {
		const world = good();
		spoil(world);
		const conditions = failing(world);
		// No served head means no fetch, so migrations can't be read either.
		expect(conditions).toEqual(world.served ? [name] : [name, "migrations"]);
		const text = render(check(PR, fake(world).run, { root: ROOT, deliversCheck: PARSER }));
		expect(text).toContain("NOT FINAL");
		expect(text).toMatch(new RegExp(`- ${name}: .*Next: `));
	});

	it("lets a newer pass supersede an older failure, by job id", () => {
		const world = good();
		world.rollup = [
			{ ...pass("rust-test", 100), conclusion: "FAILURE" },
			pass("rust-test", 101),
			pass("rust-lint"),
		];
		expect(failing(world)).toEqual([]);
	});

	it("treats a newer in-progress or queued run as not final, whatever its timestamps say", () => {
		// GitHub leaves startedAt null on a queued run; absent, empty and zero
		// sentinels must not lose to an older pass.
		for (const startedAt of [undefined, null, "", "0001-01-01T00:00:00Z"]) {
			for (const status of ["IN_PROGRESS", "QUEUED"]) {
				const world = good();
				world.rollup = [
					pass("rust-test", 100),
					{ ...pass("rust-test", 101), status, conclusion: null, startedAt },
				];
				expect(failing(world)).toEqual(["checks"]);
				const text = render(check(PR, fake(world).run, { root: ROOT, deliversCheck: PARSER }));
				expect(text).toContain("Next: wait for ci / rust-test");
			}
		}
		expect(checkState({ __typename: "StatusContext", context: "x", state: "PENDING" })).toBe(
			"running",
		);
	});

	it("orders a real re-run (attempt 2) by job id, and keeps two workflows' same-named checks apart", () => {
		const rerun = good();
		// Attempt 1 failed, attempt 2 (same run, a newer job) passed; newest listed first.
		rerun.rollup = [
			pass("rust-test", 205),
			{ ...pass("rust-test", 200), conclusion: "FAILURE" },
			pass("rust-lint"),
		];
		expect(failing(rerun)).toEqual([]);
		const two = good();
		two.rollup = [pass("lint", 100, "ci"), { ...pass("lint", 300, "docs"), conclusion: "FAILURE" }];
		expect(failing(two)).toEqual(["checks"]);
		expect([...latestChecks(two.rollup).keys()]).toEqual(["ci / lint", "docs / lint"]);
	});

	it("fails closed when same-named runs can't be ordered", () => {
		const noId = { ...pass("rust-test"), detailsUrl: null };
		const unordered = good();
		unordered.rollup = [noId, { ...noId, conclusion: "FAILURE" }];
		expect(failing(unordered)).toEqual(["checks"]);
		expect(render(check(PR, fake(unordered).run, { root: ROOT, deliversCheck: PARSER }))).toContain(
			"can't be ordered",
		);
		const pending = good();
		pending.rollup = [noId, { ...noId, status: "QUEUED", conclusion: null }];
		expect(latestChecks(pending.rollup).get("ci / rust-test")?.state).toBe("running");
		// One run with no id is still that run.
		const single = good();
		single.rollup = [noId];
		expect(failing(single)).toEqual([]);
	});

	it("numbers migrations above main's highest, with SCHEMA_VERSION at the top one", () => {
		const above = good();
		above.added = [40];
		above.headSchema = 40;
		expect(failing(above)).toEqual([]);
		const at = good();
		at.mainMigrations = [36, 38, 39];
		at.added = [37];
		at.headSchema = 37;
		expect(failing(at)).toEqual(["migrations"]);
		expect(render(check(PR, fake(at).run, { root: ROOT }))).toContain(
			"37 not above main's highest 39",
		);
		const stale = good();
		stale.added = [40];
		stale.headSchema = 36;
		expect(failing(stale)).toEqual(["migrations"]);
		expect(
			migrationNumbers(
				"crates/store/migrations/0011_a.up.sql\ncrates/store/migrations/0011_a.down.sql",
			),
		).toEqual([11]);
	});

	it("without the parser, needs CI's delivers check present and passed", () => {
		const prose = good();
		prose.body = "Delivers: the s05 send path, roughly\n";
		expect(failing(prose, PARSER)).toEqual(["delivers"]);
		// No parser and no delivers check: not final.
		expect(failing(prose, null)).toEqual(["delivers"]);
		expect(render(check(PR, fake(prose).run, { root: ROOT }))).toContain(
			"CI's delivers check is absent",
		);
		// No parser, and the delivers check failed or is still running: not final.
		prose.rollup = [
			...good().rollup,
			{ ...pass("delivers", 100, "delivers"), conclusion: "FAILURE" },
		];
		expect(failing(prose, null)).toEqual(["checks", "delivers"]);
		prose.rollup = [
			...good().rollup,
			{ ...pass("delivers", 100, "delivers"), status: "QUEUED", conclusion: null },
		];
		expect(failing(prose, null)).toEqual(["checks", "delivers"]);
		// No parser, and the delivers check passed: it stands in.
		const passed = good();
		passed.rollup = [...good().rollup, pass("delivers", 100, "delivers")];
		expect(failing(passed, null)).toEqual([]);
		expect(render(check(PR, fake(passed).run, { root: ROOT }))).toContain(
			"CI's delivers check succeeded",
		);
	});

	it("reports a review worktree only when detached at exactly this head, and clean", () => {
		const at = `/work/review-${PR}-${HEAD.slice(0, 7)}`;
		const notes = (world: World) => check(PR, fake(world).run, { root: ROOT }).notes.join("\n");
		const clean = good();
		clean.worktrees += `\nworktree ${at}\nHEAD ${HEAD}\ndetached\n`;
		expect(notes(clean)).toContain(`worktree  ${at} (detached at ${HEAD.slice(0, 7)}, clean)`);
		const stale = good();
		stale.worktrees += `\nworktree ${at}\nHEAD ${OTHER}\ndetached\n`;
		expect(notes(stale)).toContain(`not usable: ${at} is at ${OTHER.slice(0, 7)}`);
		const branch = good();
		branch.worktrees += `\nworktree ${at}\nHEAD ${HEAD}\nbranch refs/heads/review\n`;
		expect(notes(branch)).toContain(`not usable: ${at} is on a branch, not detached`);
		const dirty = { ...clean, dirty: [at] };
		expect(notes(dirty)).toContain(`not usable: ${at} has local changes`);
		expect(notes(good())).toContain(`Create one: just pr-freeze ${PR} --worktree`);
	});

	it("--worktree creates one at the ls-remote head, reuses only a clean detached one, refuses otherwise", () => {
		const path = `/work/review-${PR}-${HEAD.slice(0, 7)}`;
		const { run, calls } = fake(good());
		expect(createWorktree(check(PR, run, { root: ROOT }), run, { root: ROOT })).toBe(path);
		expect(calls).toContain(`git worktree add --detach ${path} ${HEAD}`);

		const existing = good();
		existing.worktrees += `\nworktree ${path}\nHEAD ${HEAD}\ndetached\n`;
		const reuse = fake(existing);
		expect(createWorktree(check(PR, reuse.run, { root: ROOT }), reuse.run, { root: ROOT })).toBe(
			path,
		);
		expect(reuse.calls.some((call) => call.startsWith("git worktree add"))).toBe(false);

		const spoils: ((world: World) => void)[] = [
			(w) => {
				w.worktrees += `\nworktree ${path}\nHEAD ${HEAD}\nbranch refs/heads/review\n`;
			},
			(w) => {
				w.worktrees += `\nworktree ${path}\nHEAD ${HEAD}\ndetached\n`;
				w.dirty = [path];
			},
			(w) => {
				w.worktrees += `\nworktree ${path}\nHEAD ${OTHER}\ndetached\n`;
			},
			(w) => {
				w.ghHead = OTHER;
			},
		];
		for (const spoil of spoils) {
			const world = good();
			spoil(world);
			const attempt = fake(world);
			expect(() =>
				createWorktree(check(PR, attempt.run, { root: ROOT }), attempt.run, { root: ROOT }),
			).toThrow(/refusing --worktree/);
			expect(attempt.calls.some((call) => call.startsWith("git worktree add"))).toBe(false);
		}
	});

	it("without the parser, a SKIPPED or NEUTRAL delivers run is not FINAL; only SUCCESS stands in", () => {
		for (const conclusion of ["SKIPPED", "NEUTRAL"]) {
			const world = good();
			world.body = "Delivers: the s05 send path, roughly\n";
			world.rollup = [...good().rollup, { ...pass("delivers", 100, "delivers"), conclusion }];
			// The generic checks condition still accepts it; the Delivers fallback doesn't.
			expect(failing(world, null)).toEqual(["delivers"]);
			const text = render(check(PR, fake(world).run, { root: ROOT }));
			expect(text).toContain(`CI's delivers check is ${conclusion}, not SUCCESS`);
			expect(text).toContain("Next: build the parser");
		}
		const success = good();
		success.rollup = [...good().rollup, pass("delivers", 100, "delivers")];
		expect(failing(success, null)).toEqual([]);
	});

	it("never calls a PR ready without positive evidence: an absent isDraft is not ready", () => {
		const world = good() as Omit<World, "draft"> & { draft?: boolean };
		world.draft = undefined;
		expect(failing(world as World)).toEqual(["draft"]);
		expect(
			render(check(PR, fake(world as World).run, { root: ROOT, deliversCheck: PARSER })),
		).toContain("draft state not reported");
	});

	it("a failed path lookup prints a diagnostic and exits 2, never nothing and 0", () => {
		const url = pathToFileURL(SCRIPT).href;
		const decided = entry(url, SCRIPT, () => {
			throw new Error("EACCES: permission denied");
		});
		expect(decided.kind).toBe("error");
		const calls: string[] = [];
		guard(
			decided,
			() => {
				calls.push("run");
				return 0;
			},
			(code) => calls.push(`exit ${code}`),
			(message) => calls.push(message),
		);
		expect(calls).toHaveLength(2);
		expect(calls[0]).toContain(
			"could not tell whether it was run as a script (EACCES: permission denied)",
		);
		expect(calls[1]).toBe("exit 2");
		// A proven import does nothing; a proven run exits by main's result.
		const importCalls: string[] = [];
		guard(
			{ kind: "import" },
			() => 0,
			(code) => importCalls.push(`exit ${code}`),
			(message) => importCalls.push(message),
		);
		expect(importCalls).toEqual([]);
		const runCalls: string[] = [];
		guard(
			{ kind: "run" },
			() => 1,
			(code) => runCalls.push(`exit ${code}`),
			(message) => runCalls.push(message),
		);
		expect(runCalls).toEqual(["exit 1"]);
	});

	it("runs, prints and exits by its result when invoked through a symlink", () => {
		const dir = mkdtempSync(join(tmpdir(), "pr-freeze-"));
		try {
			const link = join(dir, "pr-freeze.ts");
			symlinkSync(SCRIPT, link);
			const url = pathToFileURL(SCRIPT).href;
			expect(entry(url, link)).toEqual({ kind: "run" });
			expect(entry(url, SCRIPT)).toEqual({ kind: "run" });
			expect(entry(url, dir)).toEqual({ kind: "import" });
			expect(entry(url, undefined)).toEqual({ kind: "import" });
			const [node = process.execPath, ...flags] = NODE;
			const result = spawnSync(node, [...flags, link], { encoding: "utf8" });
			expect(result.status).toBe(2);
			expect(result.stderr).toContain("usage: just pr-freeze <pr number>");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});

describe("pr-freeze --prune (F107)", () => {
	const SHA = HEAD;
	const at = (path: string, head = SHA, detached = true) =>
		`worktree ${path}\nHEAD ${head}\n${detached ? "detached" : "branch refs/heads/x"}\n`;
	const candidate = (over: Partial<PruneCandidate> = {}): PruneCandidate => ({
		path: `/work/review-7-${SHA.slice(0, 7)}`,
		pr: 7,
		sha7: SHA.slice(0, 7),
		head: SHA,
		detached: true,
		...over,
	});

	it("only the PR's state decides; local changes and use only ever spare", () => {
		const merged = { state: "MERGED", headRefOid: SHA };
		const moved = { state: "OPEN", headRefOid: OTHER };
		const current = { state: "OPEN", headRefOid: SHA };
		expect(decide(candidate(), merged, undefined, undefined)).toMatchObject({
			remove: true,
			why: "PR #7 is merged",
		});
		expect(
			decide(candidate(), { state: "CLOSED", headRefOid: SHA }, undefined, undefined).remove,
		).toBe(true);
		expect(decide(candidate(), moved, undefined, undefined)).toMatchObject({
			remove: true,
			why: "PR #7's head moved on to 08f7482",
		});
		expect(decide(candidate(), current, undefined, undefined)).toMatchObject({
			remove: false,
			why: "PR #7 is open at this head",
		});
		// Brakes: each spares a worktree the state would remove, and never
		// turns a keep into a remove.
		for (const [dirty, busy] of [
			["it has local changes", undefined],
			[undefined, "seat pij-x's folder is in it"],
		] as const) {
			expect(decide(candidate(), merged, dirty, busy).remove).toBe(false);
			expect(decide(candidate(), current, dirty, busy).remove).toBe(false);
		}
		expect(decide(candidate(), undefined, undefined, undefined).remove).toBe(false);
		// F001: decide spares a state it doesn't know, or an OPEN head that isn't a full sha.
		for (const pr of [
			{ state: "UNKNOWN", headRefOid: OTHER },
			{ state: "OPEN", headRefOid: "" },
			{ state: "OPEN", headRefOid: "08f7482" },
		]) {
			expect(decide(candidate(), pr, undefined, undefined).remove).toBe(false);
		}
		expect(decide(candidate({ detached: false }), merged, undefined, undefined).remove).toBe(false);
		expect(decide(candidate({ head: OTHER }), merged, undefined, undefined).remove).toBe(false);
	});

	it("validates gh's PR before anything uses it (bobcat F001)", () => {
		expect(prState(JSON.stringify({ state: "MERGED", headRefOid: SHA }))).toEqual({
			state: "MERGED",
			headRefOid: SHA,
		});
		expect(prState(JSON.stringify({ state: "CLOSED" }))).toEqual({
			state: "CLOSED",
			headRefOid: "",
		});
		expect(prState(JSON.stringify({ state: "OPEN", headRefOid: OTHER }))).toEqual({
			state: "OPEN",
			headRefOid: OTHER,
		});
		for (const stdout of [
			JSON.stringify({ state: "OPEN" }),
			JSON.stringify({ state: "OPEN", headRefOid: "08f7482" }),
			JSON.stringify({}),
			JSON.stringify({ state: "UNKNOWN", headRefOid: OTHER }),
			"null",
			"not json",
		]) {
			expect(prState(stdout), stdout).toBe(undefined);
		}
	});

	interface Fleet {
		worktrees: string;
		prs: Record<number, { state: string; headRefOid: string } | null>;
		dirty: string[];
		seats: { id: string; folder: string }[] | null;
		slow: string[];
	}

	function fleet(world: Fleet): { run: Runner; calls: string[] } {
		const calls: string[] = [];
		const run: Runner = (program, args) => {
			const call = [program, ...args].join(" ");
			calls.push(call);
			if (world.slow.some((slow) => call.startsWith(slow))) {
				return { code: 127, stdout: "", stderr: "timed out after 60s", timedOut: true };
			}
			if (program === "pij-rs") {
				return world.seats
					? ok(JSON.stringify({ ok: true, data: { seats: world.seats } }))
					: { code: 1, stdout: "", stderr: "daemon down" };
			}
			if (program === "gh") {
				const pr = world.prs[Number(args[2])];
				return pr ? ok(JSON.stringify(pr)) : { code: 1, stdout: "", stderr: "no PR" };
			}
			if (args[0] === "-C") return ok(world.dirty.includes(args[1] ?? "") ? "?? notes.md\n" : "");
			if (args[0] === "worktree" && args[1] === "list") return ok(world.worktrees);
			if (args[0] === "worktree" && args[1] === "remove") return ok();
			return { code: 1, stdout: "", stderr: `unexpected ${program} ${args.join(" ")}` };
		};
		return { run, calls };
	}

	/** Real directories, so realpath resolves (an unresolvable one is spared). */
	function dirs(): { root: string; path: (pr: number) => string; done: () => void } {
		const root = realpathSync(mkdtempSync(join(tmpdir(), "pr-freeze-prune-")));
		const path = (pr: number) => {
			const dir = join(root, `review-${pr}-${SHA.slice(0, 7)}`);
			mkdirSync(dir, { recursive: true });
			return dir;
		};
		return { root, path, done: () => rmSync(root, { recursive: true, force: true }) };
	}

	function world(path: (pr: number) => string, root: string): Fleet {
		return {
			worktrees: [
				`worktree ${ROOT}\nHEAD ${MAIN}\nbranch refs/heads/main\n`,
				...[7, 8, 9, 10, 11].map((pr) => at(path(pr))),
				at(join(root, `review-12-${SHA.slice(0, 7)}`)),
				at("/work/fleet-review-113"),
			].join("\n"),
			prs: {
				7: { state: "MERGED", headRefOid: SHA },
				8: { state: "OPEN", headRefOid: SHA },
				9: { state: "CLOSED", headRefOid: SHA },
				10: { state: "OPEN", headRefOid: OTHER },
				11: { state: "MERGED", headRefOid: SHA },
				12: { state: "MERGED", headRefOid: SHA },
			},
			dirty: [path(9)],
			seats: [{ id: "pij-reviewer", folder: `${path(10)}/crates` }],
			slow: [],
		};
	}

	it("removes only worktrees whose work is over and that nothing is using", () => {
		const { root, path, done } = dirs();
		try {
			// The process sits in #11's worktree through a symlinked spelling.
			const link = join(root, "link-11");
			symlinkSync(path(11), link);
			const { run, calls } = fleet(world(path, root));
			const verdicts = prune(run, false, () => [{ pid: "4242", cwd: link }], "darwin");
			expect(verdicts.map((verdict) => [basename(verdict.path), verdict.remove])).toEqual([
				["review-7-a7a0f4a", true],
				["review-8-a7a0f4a", false],
				["review-9-a7a0f4a", false],
				["review-10-a7a0f4a", false],
				["review-11-a7a0f4a", false],
				["review-12-a7a0f4a", false],
			]);
			expect(calls.filter((call) => call.startsWith("git worktree remove"))).toEqual([
				`git worktree remove ${path(7)}`,
			]);
			const text = renderPrune(verdicts, false);
			expect(text).toContain("1 of 6 removed");
			// The policy also removes an OPEN PR's worktree whose head moved on;
			// here a seat's folder spares it.
			expect(text).toContain(
				"PR #10's head moved on to 08f7482, but seat pij-reviewer's folder is in it",
			);
			expect(text).toContain("PR #9 is closed, but it has local changes");
			expect(text).toContain("PR #11 is merged, but process 4242's cwd is in it");
			expect(text).toContain(
				"PR #12 is merged, but its path doesn't resolve, so pij can't tell who is in it",
			);
			expect(text).not.toContain("fleet-review-113");
		} finally {
			done();
		}
	});

	it("reads everything first; an unreadable seat list, cwd listing or a timeout removes nothing", () => {
		const { root, path, done } = dirs();
		try {
			const none = () => [];
			const dry = fleet(world(path, root));
			expect(renderPrune(prune(dry.run, true, none, "darwin"), true)).toContain(
				`would remove ${path(7)}`,
			);
			expect(dry.calls.some((call) => call.startsWith("git worktree remove"))).toBe(false);

			for (const broken of [
				{ ...world(path, root), seats: null },
				{ ...world(path, root), slow: ["gh pr view 9"] },
				{ ...world(path, root), slow: [`git -C ${path(11)} status`] },
			]) {
				const attempt = fleet(broken);
				expect(() => prune(attempt.run, false, none, "darwin")).toThrow(
					/refusing --prune: .*; nothing was removed/,
				);
				expect(attempt.calls.some((call) => call.startsWith("git worktree remove"))).toBe(false);
			}
			const blind = fleet(world(path, root));
			expect(() =>
				prune(
					blind.run,
					false,
					() => {
						throw new Error("lsof did not list every cwd");
					},
					"darwin",
				),
			).toThrow(/refusing --prune: can't tell which worktrees are in use/);
			expect(blind.calls.some((call) => call.startsWith("git worktree remove"))).toBe(false);
		} finally {
			done();
		}
	});

	it("spares a PR gh reports unusably and refuses a malformed seat row (bobcat F001, F002)", () => {
		const { root, path, done } = dirs();
		try {
			const none = () => [];
			// One unused worktree of PR 7; only gh's answer for it varies.
			const only = (prs: Fleet["prs"], seats: Fleet["seats"] = []): Fleet => ({
				...world(path, root),
				worktrees: at(path(7)),
				prs,
				seats,
			});
			for (const payload of [
				{ state: "OPEN" },
				{},
				{ state: "UNKNOWN", headRefOid: OTHER },
				{ state: "OPEN", headRefOid: "08f7482" },
				{ state: "open", headRefOid: OTHER },
			]) {
				const attempt = fleet(only({ 7: payload as { state: string; headRefOid: string } }));
				const [verdict] = prune(attempt.run, false, none, "darwin");
				expect(verdict?.remove, JSON.stringify(payload)).toBe(false);
				expect(attempt.calls.some((call) => call.startsWith("git worktree remove"))).toBe(false);
			}
			for (const seat of [
				{ id: "pij-reviewer" },
				{ id: "pij-reviewer", folder: 7 },
				{ id: "pij-reviewer", folder: "" },
				{ folder: path(7) },
				null,
			]) {
				const attempt = fleet(
					only({ 7: { state: "MERGED", headRefOid: SHA } }, [
						seat as unknown as { id: string; folder: string },
					]),
				);
				expect(() => prune(attempt.run, false, none, "darwin"), JSON.stringify(seat)).toThrow(
					/refusing --prune: can't tell which worktrees are in use \(pij-rs list seat 0 has no usable id and folder/,
				);
				expect(attempt.calls.some((call) => call.startsWith("git worktree remove"))).toBe(false);
			}
			// The positive control: the same worktree, a merged PR and sound rows, is removed.
			const sound = fleet(
				only({ 7: { state: "MERGED", headRefOid: SHA } }, [{ id: "pij-x", folder: "/elsewhere" }]),
			);
			expect(prune(sound.run, false, none, "darwin")[0]?.remove).toBe(true);
		} finally {
			done();
		}
	});

	it("refuses an lsof listing that failed, said anything on stderr, or doesn't parse (S170-001)", () => {
		const lsof =
			(stdout: string, code = 0, stderr = ""): Runner =>
			() => ({ code, stdout, stderr });
		const good = "p12\nfcwd\nn/work/a\np13\nfcwd\nn/work/b\n";
		expect(processCwds(lsof(good))).toEqual([
			{ pid: "12", cwd: "/work/a" },
			{ pid: "13", cwd: "/work/b" },
		]);
		for (const [stdout, code, stderr] of [
			[good, 1, "lsof: denied"],
			[good, 0, "lsof: WARNING: can't stat() nfs file system"],
			[good, 127, "timed out after 60s"],
			["n/work/a\n", 0, ""],
			["p123\n", 0, ""],
			["p12\nfcwd\nn/work/a\np13\n", 0, ""],
			["p12\nfcwd\nn/work/a\np13\nfcwd\np14\nfcwd\nn/work/c\n", 0, ""],
			["p12\nfcwd\nn/work/a\nn/work/b\n", 0, ""],
			["p12\nx?\nn/work/a\n", 0, ""],
			["p12\nn\n", 0, ""],
			["", 0, ""],
		] as const) {
			expect(() => processCwds(lsof(stdout, code, stderr))).toThrow();
		}
	});

	it("refuses on any platform but macOS, before reading or removing anything", () => {
		const { root, path, done } = dirs();
		try {
			for (const platform of ["linux", "win32", "freebsd"]) {
				const attempt = fleet(world(path, root));
				expect(() => prune(attempt.run, false, () => [], platform)).toThrow(MACOS_ONLY);
				expect(attempt.calls).toEqual([]);
			}
		} finally {
			done();
		}
	});

	it("launches git, gh and pij-rs by absolute paths resolved once (P170-F001)", () => {
		const probed: string[] = [];
		const tools = resolveTools(["git", "gh", "pij-rs"], "relative:/opt/a:/usr/bin", (file) => {
			probed.push(file);
			return file === "/usr/bin/git" || file === "/opt/a/gh" || file === "/opt/a/pij-rs";
		});
		expect([...tools]).toEqual([
			["git", "/usr/bin/git"],
			["gh", "/opt/a/gh"],
			["pij-rs", "/opt/a/pij-rs"],
		]);
		expect(probed.some((file) => file.startsWith("relative"))).toBe(false);
		const resolved = probed.length;
		const launched: string[] = [];
		const run = resolvedRunner(tools, (program) => {
			launched.push(program);
			return ok();
		});
		for (let i = 0; i < 3; i++) {
			run("git", ["status"]);
			run("gh", ["pr", "view", "7"]);
		}
		run("/usr/sbin/lsof", ["-d", "cwd"]);
		expect(launched).toEqual([
			"/usr/bin/git",
			"/opt/a/gh",
			"/usr/bin/git",
			"/opt/a/gh",
			"/usr/bin/git",
			"/opt/a/gh",
			"/usr/sbin/lsof",
		]);
		expect(probed.length).toBe(resolved);
		expect(run("ps", []).code).toBe(127);
		expect(launched).not.toContain("ps");
		expect(() => resolveTools(["git", "gh"], "/nowhere", () => false)).toThrow(
			"git, gh not found on PATH",
		);
	});
});
