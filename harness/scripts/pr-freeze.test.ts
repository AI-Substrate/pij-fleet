import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";
import {
	check,
	checkState,
	createWorktree,
	entry,
	guard,
	latestChecks,
	migrationNumbers,
	type Ran,
	type Runner,
	render,
} from "./pr-freeze.js";

const SCRIPT = resolve(import.meta.dirname, "pr-freeze.ts");
const TSX = resolve(import.meta.dirname, "..", "..", "node_modules", ".bin", "tsx");

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
			const result = spawnSync(TSX, [link], { encoding: "utf8" });
			expect(result.status).toBe(2);
			expect(result.stderr).toContain("usage: just pr-freeze <pr number>");
		} finally {
			rmSync(dir, { recursive: true, force: true });
		}
	});
});
