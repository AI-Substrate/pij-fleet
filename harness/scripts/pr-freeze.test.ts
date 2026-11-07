import { describe, expect, it } from "vitest";
import {
	check,
	checkState,
	createWorktree,
	latestChecks,
	migrationNumbers,
	type Ran,
	type Runner,
	render,
} from "./pr-freeze.js";

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
}

const pass = (name: string, startedAt = "2026-11-07T14:00:00Z") => ({
	__typename: "CheckRun",
	name,
	workflowName: "ci",
	status: "COMPLETED",
	conclusion: "SUCCESS",
	startedAt,
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

	it("lets a newer pass supersede an older failure, by check name", () => {
		const world = good();
		world.rollup = [
			{ ...pass("rust-test", "2026-11-07T13:00:00Z"), conclusion: "FAILURE" },
			pass("rust-test", "2026-11-07T14:00:00Z"),
			pass("rust-lint"),
		];
		expect(failing(world)).toEqual([]);
	});

	it("treats a newer in-progress or queued run as not final, even over an older pass", () => {
		for (const status of ["IN_PROGRESS", "QUEUED"]) {
			const world = good();
			world.rollup = [
				pass("rust-test", "2026-11-07T13:00:00Z"),
				{ ...pass("rust-test", "2026-11-07T14:00:00Z"), status, conclusion: null },
			];
			expect(failing(world)).toEqual(["checks"]);
			const text = render(check(PR, fake(world).run, { root: ROOT, deliversCheck: PARSER }));
			expect(text).toContain("Next: wait for ci / rust-test");
		}
		expect(checkState({ __typename: "StatusContext", context: "x", state: "PENDING" })).toBe(
			"running",
		);
		expect(latestChecks([pass("a"), pass("a")]).size).toBe(1);
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

	it("defers to CI's delivers check only when the parser isn't built", () => {
		const prose = good();
		prose.body = "Delivers: the s05 send path, roughly\n";
		expect(failing(prose, PARSER)).toEqual(["delivers"]);
		expect(failing(prose, null)).toEqual([]);
		const text = render(check(PR, fake(prose).run, { root: ROOT }));
		expect(text).toContain("CI's delivers check stands in");
	});

	it("reports a review worktree only at exactly this head, named -<pr>-<head7>", () => {
		const world = good();
		const at = `/work/review-${PR}-${HEAD.slice(0, 7)}`;
		world.worktrees += `\nworktree ${at}\nHEAD ${HEAD}\ndetached\n`;
		const report = check(PR, fake(world).run, { root: ROOT });
		expect(report.notes.join("\n")).toContain(`worktree  ${at}`);
		const stale = good();
		stale.worktrees += `\nworktree ${at}\nHEAD ${OTHER}\ndetached\n`;
		expect(check(PR, fake(stale).run, { root: ROOT }).notes.join("\n")).toContain(
			`Create one: just pr-freeze ${PR} --worktree`,
		);
	});

	it("--worktree creates it detached at the ls-remote head, reuses one, and refuses on a head mismatch", () => {
		const { run, calls } = fake(good());
		const report = check(PR, run, { root: ROOT });
		const path = createWorktree(report, run, { root: ROOT });
		expect(path).toBe(`/work/review-${PR}-${HEAD.slice(0, 7)}`);
		expect(calls).toContain(`git worktree add --detach ${path} ${HEAD}`);

		const existing = good();
		existing.worktrees += `\nworktree ${path}\nHEAD ${HEAD}\ndetached\n`;
		const reuse = fake(existing);
		expect(createWorktree(check(PR, reuse.run, { root: ROOT }), reuse.run, { root: ROOT })).toBe(
			path,
		);
		expect(reuse.calls.some((call) => call.startsWith("git worktree add"))).toBe(false);

		const moved = good();
		moved.ghHead = OTHER;
		const mismatch = fake(moved);
		expect(() =>
			createWorktree(check(PR, mismatch.run, { root: ROOT }), mismatch.run, { root: ROOT }),
		).toThrow(/refusing --worktree/);
		expect(mismatch.calls.some((call) => call.startsWith("git worktree add"))).toBe(false);
	});
});
