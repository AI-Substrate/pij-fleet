import {
	chmodSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	CHECK_TIMEOUT_MS,
	commandItems,
	EXIT_FAIL,
	EXIT_OK,
	EXIT_REFUSED,
	EXIT_TIMED_OUT,
	EXIT_USAGE,
	type ExecResult,
	type Io,
	main,
	pathsIn,
	podDown,
	progressDir,
	RELAYED_MESSAGE,
	readProgress,
	realIo,
	recipesIn,
	recordOf,
	runFile,
	saveRecord,
	splitWords,
} from "./procedures.ts";

interface Call {
	file: string;
	args: string[];
	env: Record<string, string | undefined>;
	timeoutMs?: number;
}

/** A recipe's behaviour in the fake `just`: its exit status, from the env it was given. */
type Recipe = (env: Record<string, string | undefined>) => number;

let root: string;
let bin: string;
let calls: Call[];
let recipes: Record<string, Recipe>;
let out: string[];
let err: string[];
let claimNumber: number;
/** What the fake `pij-rs pod show` answers with. */
let podShown: Record<string, unknown>;

function write(path: string, text: string): void {
	mkdirSync(dirname(join(root, path)), { recursive: true });
	writeFileSync(join(root, path), text);
}

function listFiles(dir: string): string[] {
	const files: string[] = [];
	for (const name of readdirSync(dir)) {
		if (name === ".git" || name === ".pij") continue;
		const path = join(dir, name);
		if (statSync(path).isDirectory()) files.push(...listFiles(path));
		else files.push(relative(root, path));
	}
	return files;
}

function fakeExec(
	file: string,
	args: string[],
	_cwd: string,
	env: Record<string, string | undefined>,
	_inherit: boolean,
	timeoutMs?: number,
): ExecResult {
	calls.push({ file: basename(file), args, env, timeoutMs });
	const tool = basename(file);
	if (tool === "git" && args[0] === "ls-files")
		return { status: 0, stdout: `${listFiles(root).join("\0")}\0`, stderr: "" };
	if (tool === "git") return { status: 0, stdout: "", stderr: "" };
	if (tool === "pij")
		return {
			status: 0,
			stdout: `${args[1]} ${String(claimNumber).padStart(3, "0")} claimed by pij-test\n`,
			stderr: "",
		};
	if (tool === "ddocs") return { status: 0, stdout: "{}", stderr: "" };
	if (tool === "pij-rs" && args.includes("show"))
		return { status: 0, stdout: JSON.stringify({ ok: true, data: podShown }), stderr: "" };
	if (tool === "pij-rs") return { status: 0, stdout: "", stderr: "" };
	if (tool === "just") {
		const justfile = args[args.indexOf("--justfile") + 1] ?? "";
		if (args.includes("--summary"))
			return {
				status: 0,
				stdout: [...recipesIn(readFileSync(justfile, "utf8"))].join(" "),
				stderr: "",
			};
		const recipe = args[args.indexOf("--working-directory") + 2] ?? "";
		const behaviour = recipes[recipe];
		return { status: behaviour ? behaviour(env) : 0, stdout: "", stderr: behaviour ? "" : "" };
	}
	return { status: 127, stdout: "", stderr: `unexpected ${tool}` };
}

function io(cwd = root, env: Record<string, string | undefined> = {}): Io {
	return {
		cwd,
		env: { PATH: bin, PIJ_SESSION_ID: "pij-test", ...env },
		out: (line) => out.push(line),
		err: (line) => err.push(line),
		exec: fakeExec,
		now: () => new Date("2026-11-24T10:00:00Z"),
	};
}

function run(
	argv: string[],
	options: { cwd?: string; env?: Record<string, string | undefined> } = {},
): number {
	return main(argv, io(options.cwd, options.env), { skillHome: join(root, "no-skill-home") });
}

function runsOf(recipe: string): Call[] {
	return calls.filter(
		(c) =>
			c.file === "just" &&
			!c.args.includes("--summary") &&
			c.args[c.args.indexOf("--working-directory") + 2] === recipe,
	);
}

function procedure(
	area: string,
	number: string,
	slug: string,
	sections: Record<string, unknown>,
	justfile?: string,
	home = "docs/project/procedures",
): string {
	const id = `PROC-${area}-${number}`;
	const dir = `${home}/${area}/${id}-${slug}`;
	const meta = {
		id,
		title: `${slug} title`,
		area,
		kind: "runnable",
		status: "draft",
		owner: "pm-001",
		...((sections.meta as object) ?? {}),
	};
	const all: Record<string, unknown> = {
		who_when: { who: "a seat", when: "on demand" },
		done_when: "it is done",
		...sections,
		meta,
	};
	write(
		`${dir}/procedure.dd.json`,
		JSON.stringify({
			dd: { schema: "pij/procedure" },
			sections: Object.entries(all).map(([name, value]) => ({ name, value })),
			references: [],
		}),
	);
	if (justfile !== undefined) write(`${dir}/justfile`, justfile);
	return dir;
}

const THREE_STEPS = {
	steps: [
		{
			id: "s0001",
			title: "first",
			kind: "run",
			run: "one",
			done: "one-done",
			done_when: "one is there",
		},
		{ id: "s0002", title: "second", kind: "run", run: "two", done_when: "two is there" },
		{ id: "s0003", title: "third", kind: "run", run: "three", done_when: "three is there" },
	],
};
const THREE_RECIPES = "one:\n    true\none-done:\n    true\ntwo:\n    true\nthree:\n    true\n";

beforeEach(() => {
	root = mkdtempSync(join(tmpdir(), "pij-proc-"));
	bin = join(root, "bin-stubs");
	mkdirSync(bin);
	for (const tool of ["just", "git", "pij", "ddocs"]) {
		writeFileSync(join(bin, tool), "#!/bin/sh\nexit 99\n");
		chmodSync(join(bin, tool), 0o755);
	}
	// A linked worktree on a feature branch.
	const gitDir = join(root, "gitdir");
	mkdirSync(gitDir);
	writeFileSync(join(gitDir, "HEAD"), "ref: refs/heads/feature\n");
	writeFileSync(join(root, ".git"), `gitdir: ${gitDir}\n`);
	write("justfile", "proc *ARGS:\n    true\nshared:\n    true\n");
	calls = [];
	recipes = {};
	out = [];
	err = [];
	claimNumber = 1;
	podShown = {};
});

afterEach(() => rmSync(root, { recursive: true, force: true }));

describe("parsing helpers", () => {
	it("reads recipe names, aliases and parameters but not variables or settings", () => {
		const names = recipesIn(
			'set positional-arguments\nx := "1"\n@quiet:\n    true\nproc *ARGS:\n    true\nwith a="b": quiet\nalias q := quiet\n# c:\n',
		);
		expect([...names].sort()).toEqual(["proc", "q", "quiet", "with"]);
	});

	it("splits words with quotes", () => {
		expect(splitWords(`step "a b" 'c d' e`)).toEqual(["step", "a b", "c d", "e"]);
	});

	it("counts numbered items that begin with a command, outside code blocks", () => {
		const page = [
			"1. `just install`",
			"2. **Build.**",
			"   ```bash",
			"   just build",
			"   ```",
			"3. Read the result.",
			"```",
			"4. `not counted, inside a fence`",
			"```",
			"5. `pij list`",
		].join("\n");
		expect(commandItems(page)).toBe(3);
	});

	it("finds tracked paths, also behind a variable prefix", () => {
		const tracked = new Set(["harness/scripts/a.sh", "docs/x.md"]);
		expect(
			[...pathsIn("run $PROC_ROOT/harness/scripts/a.sh and ./docs/x.md, not a/b", tracked)].sort(),
		).toEqual(["docs/x.md", "harness/scripts/a.sh"]);
	});
});

describe("listing", () => {
	it("lists procedures from the files, from any directory", () => {
		const dir = procedure("fleet", "0001", "change-effort", THREE_STEPS, THREE_RECIPES);
		expect(run(["list"])).toBe(EXIT_OK);
		const fromRoot = [...out];
		out = [];
		expect(run([], { cwd: join(root, dir) })).toBe(EXIT_OK);
		expect(out).toEqual(fromRoot);
		expect(fromRoot.join("\n")).toContain("PROC-fleet-0001");
	});

	it("shows a run's steps from progress alone, launching nothing", () => {
		procedure("fleet", "0001", "change-effort", THREE_STEPS, THREE_RECIPES);
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_OK);
		calls = [];
		out = [];
		expect(run(["change-effort", "--json"])).toBe(EXIT_OK);
		expect(calls).toEqual([]);
		const shown = JSON.parse(out.join("\n"));
		expect(shown.steps.map((s: { state: string }) => s.state)).toEqual(["done", "next", "todo"]);
		expect(shown.next).toBe("s0002");
	});
});

describe("running steps", () => {
	beforeEach(() => procedure("fleet", "0001", "change-effort", THREE_STEPS, THREE_RECIPES));

	it("skips a step whose done check passes, and reruns it with --again", () => {
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_OK);
		expect(runsOf("one")).toHaveLength(0);
		expect(out.join("\n")).toContain("already done (one is there) (--again to run it anyway)");
		expect(run(["PROC-fleet-0001", "s0001", "--again"])).toBe(EXIT_OK);
		expect(runsOf("one")).toHaveLength(1);
	});

	it("runs a step, then a second run without a done check is a no-op", () => {
		expect(run(["PROC-fleet-0001", "s0002"])).toBe(EXIT_OK);
		expect(runsOf("two")).toHaveLength(1);
		expect(run(["PROC-fleet-0001", "s0002"])).toBe(EXIT_OK);
		expect(runsOf("two")).toHaveLength(1);
		expect(out.join("\n")).toContain("already done (ran");
	});

	it("takes a step's digits alone as its id", () => {
		expect(run(["PROC-fleet-0001", "0002"])).toBe(EXIT_OK);
		expect(runsOf("two")).toHaveLength(1);
	});

	it("refuses a step whose earlier step isn't done, naming it", () => {
		expect(run(["PROC-fleet-0001", "s0003"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain(
			"step s0002 (second) isn't done yet: run just proc PROC-fleet-0001 s0002 first",
		);
		expect(runsOf("three")).toHaveLength(0);
	});

	it("records nothing when a step fails, or when its done check fails after it ran", () => {
		recipes["one-done"] = () => 1;
		recipes.one = () => 4;
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_FAIL);
		expect(err.join("\n")).toContain("failed (exit 4); nothing recorded");
		recipes.one = () => 0;
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_FAIL);
		expect(err.join("\n")).toContain("its done check (one-done) fails: one is there");
		expect(readProgress(root).runs["PROC-fleet-0001"]).toBeUndefined();
	});

	it("keeps runs apart by key and clears one", () => {
		expect(run(["PROC-fleet-0001", "s0002", "--key", "staging"])).toBe(EXIT_OK);
		expect(readProgress(root).runs["PROC-fleet-0001"]?.staging?.["s0002"]?.how).toBe("ran");
		expect(readProgress(root).runs["PROC-fleet-0001"]?.default).toBeUndefined();
		expect(run(["PROC-fleet-0001", "--key", "staging", "--clear"])).toBe(EXIT_OK);
		expect(readProgress(root).runs["PROC-fleet-0001"]?.staging).toBeUndefined();
	});

	it("counts a done check that exits 2, 127 or times out as not done", () => {
		for (const status of [2, 127, EXIT_TIMED_OUT]) {
			calls = [];
			recipes["one-done"] = () => status;
			expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_FAIL);
			expect(runsOf("one")).toHaveLength(1);
			expect(run(["PROC-fleet-0001", "s0002"])).toBe(EXIT_REFUSED);
		}
		expect(readProgress(root).runs["PROC-fleet-0001"]).toBeUndefined();
	});

	it("bounds every check with a timeout but never the step's own recipe", () => {
		recipes["one-done"] = () => 1;
		recipes.one = () => {
			recipes["one-done"] = () => 0;
			return 0;
		};
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_OK);
		expect(runsOf("one-done").map((c) => c.timeoutMs)).toEqual([
			CHECK_TIMEOUT_MS,
			CHECK_TIMEOUT_MS,
		]);
		expect(runsOf("one")[0]?.timeoutMs).toBeUndefined();
	});

	it("refuses a key that isn't a plain word, and never touches Object.prototype", () => {
		for (const key of ["__proto__", "constructor", "toString", "../x", "a/b", "-x", ""]) {
			expect(run(["PROC-fleet-0001", "s0002", "--key", key])).toBe(EXIT_USAGE);
		}
		expect(err.join("\n")).toContain(
			'--key takes letters, digits and . _ @ - (starting with a letter or digit), not "__proto__"',
		);
		expect(runsOf("two")).toHaveLength(0);
		expect(Object.hasOwn(Object.prototype, "s0002")).toBe(false);
		const progress = { version: 1 as const, runs: { "PROC-fleet-0001": { default: {} } } };
		expect(recordOf(progress, "PROC-fleet-0001", "default", "toString")).toBeUndefined();
		expect(recordOf(progress, "PROC-fleet-0001", "constructor", "name")).toBeUndefined();
		expect(run(["PROC-fleet-0001", "s0002", "--key", "pij-a.b@c_1"])).toBe(EXIT_OK);
	});

	it("keeps each run in its own file, so a run of one key never rewrites another's", () => {
		saveRecord(root, "PROC-fleet-0001", "a", "s0001", { at: "t", by: "x", how: "ran" });
		const before = readFileSync(runFile(root, "PROC-fleet-0001", "a"), "utf8");
		expect(run(["PROC-fleet-0001", "s0002", "--key", "b"])).toBe(EXIT_OK);
		expect(readFileSync(runFile(root, "PROC-fleet-0001", "a"), "utf8")).toBe(before);
		expect(Object.keys(readProgress(root).runs["PROC-fleet-0001"] ?? {}).sort()).toEqual([
			"a",
			"b",
		]);
	});

	it("refuses unknown flags and flags that don't apply, passing --again through", () => {
		expect(run(["PROC-fleet-0001", "s0001", "--agian"])).toBe(EXIT_USAGE);
		expect(err.join("\n")).toContain("unknown flag --agian");
		expect(run(["PROC-fleet-0001", "--again"])).toBe(EXIT_USAGE);
		expect(run(["list", "--key", "x"])).toBe(EXIT_USAGE);
	});
});

describe("inputs, preconditions and order across procedures", () => {
	it("refuses a step missing a required input, with its fallback, and passes inputs to recipes", () => {
		procedure(
			"fleet",
			"0001",
			"x",
			{
				inputs: [
					{ name: "seat", description: "the seat id", required: "yes", fallback: "ask the prime" },
					{ name: "token", description: "a secret", required: "no", secret: "yes" },
				],
				steps: [
					{
						id: "s0001",
						title: "first",
						kind: "run",
						run: "one",
						inputs: ["seat"],
						done_when: "x",
					},
				],
			},
			"one:\n    true\n",
		);
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain(
			"needs input seat (the seat id): pass --arg seat=<value>. If you don't have it: ask the prime",
		);
		expect(run(["PROC-fleet-0001", "s0001", "--arg", "token=hunter2"])).toBe(EXIT_USAGE);
		expect(err.join("\n")).toContain("input token is secret");
		expect(
			run(["PROC-fleet-0001", "s0001", "--arg", "seat=pij-a"], { env: { PROC_IN_TOKEN: "t" } }),
		).toBe(EXIT_OK);
		const env = runsOf("one")[0]?.env;
		expect(env?.PROC_IN_SEAT).toBe("pij-a");
		expect(env?.PROC_IN_TOKEN).toBe("t");
		expect(env?.PROC_ID).toBe("PROC-fleet-0001");
		expect(env?.PROC_RUNNER).toMatch(/procedures\.ts$/);
	});

	it("refuses on a failing precondition with its fix, and runs nothing", () => {
		procedure(
			"fleet",
			"0001",
			"x",
			{
				preconditions: [
					{ id: "p-1", check: "idle", text: "the seat is idle", fix: "wait for idle" },
				],
				steps: [
					{
						id: "s0001",
						title: "first",
						kind: "run",
						run: "one",
						pre: [{ check: "shared", fix: "run the shared thing" }],
						done_when: "x",
					},
				],
			},
			"one:\n    true\nidle:\n    true\n",
		);
		recipes.idle = () => 1;
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain("precondition p-1 (the seat is idle) fails");
		expect(err.join("\n")).toContain("fix: wait for idle");
		recipes.idle = () => 0;
		recipes.shared = () => 1;
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain("fix: run the shared thing");
		expect(runsOf("one")).toHaveLength(0);
	});

	it("checks another procedure's step by its live done check, wherever it ran", () => {
		procedure(
			"daemon",
			"0001",
			"bounce",
			{
				steps: [
					{
						id: "s0001",
						title: "bounce it",
						kind: "run",
						run: "bounce",
						done: "bounced",
						done_when: "up",
					},
				],
			},
			"bounce:\n    true\nbounced:\n    true\n",
		);
		procedure(
			"fleet",
			"0001",
			"x",
			{
				steps: [
					{
						id: "s0001",
						title: "first",
						kind: "run",
						run: "one",
						requires: ["../../daemon/PROC-daemon-0001-bounce/procedure.dd.json#steps/s0001"],
						done_when: "x",
					},
				],
			},
			"one:\n    true\n",
		);
		recipes.bounced = () => 1;
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain(
			"needs PROC-daemon-0001 step s0001 (bounce it) first: just proc PROC-daemon-0001 s0001",
		);
		recipes.bounced = () => 0;
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_OK);
		expect(runsOf("one")).toHaveLength(1);
	});

	it("gives another procedure's done check this run's inputs, but not a secret one", () => {
		procedure(
			"daemon",
			"0001",
			"bounce",
			{
				steps: [
					{ id: "s0001", title: "b", kind: "run", run: "bounce", done: "bounced", done_when: "up" },
				],
			},
			"bounce:\n    true\nbounced:\n    true\n",
		);
		procedure(
			"fleet",
			"0001",
			"x",
			{
				inputs: [
					{ name: "seat", description: "the seat", required: "yes" },
					{ name: "token", description: "a secret", required: "yes", secret: "yes" },
				],
				steps: [
					{
						id: "s0001",
						title: "first",
						kind: "run",
						run: "one",
						inputs: ["seat", "token"],
						requires: ["../../daemon/PROC-daemon-0001-bounce/procedure.dd.json#steps/s0001"],
						done_when: "x",
					},
				],
			},
			"one:\n    true\n",
		);
		expect(
			run(["PROC-fleet-0001", "s0001", "--arg", "seat=pij-a"], { env: { PROC_IN_TOKEN: "t" } }),
		).toBe(EXIT_OK);
		const other = runsOf("bounced")[0]?.env;
		expect(other?.PROC_IN_SEAT).toBe("pij-a");
		expect(other && "PROC_IN_TOKEN" in other).toBe(false);
		expect(runsOf("one")[0]?.env.PROC_IN_TOKEN).toBe("t");
	});
});

describe("decision, guide and commands", () => {
	it("refuses a decision step until it is recorded, naming who decides", () => {
		procedure("fleet", "0001", "x", {
			steps: [
				{
					id: "s0001",
					title: "approve",
					kind: "decision",
					decide: "operator",
					done_when: "approved",
				},
			],
		});
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain("needs a decision from operator");
		expect(run(["PROC-fleet-0001", "s0001", "--decided-by", "jordan"])).toBe(EXIT_USAGE);
		expect(run(["PROC-fleet-0001", "s0001", "--decided-by", "jordan", "--note", "yes"])).toBe(
			EXIT_OK,
		);
		expect(readProgress(root).runs["PROC-fleet-0001"]?.default?.["s0001"]).toMatchObject({
			by: "jordan",
			how: "decided",
			note: "yes",
		});
	});

	it("finishes a guide step when the procedure it follows is done", () => {
		procedure(
			"daemon",
			"0001",
			"bounce",
			{ steps: [{ id: "s0001", title: "bounce", kind: "run", run: "bounce", done_when: "up" }] },
			"bounce:\n    true\n",
		);
		procedure("fleet", "0001", "x", {
			meta: { kind: "guide" },
			steps: [
				{
					id: "s0001",
					title: "bounce first",
					kind: "guide",
					follow: "PROC-daemon-0001",
					done_when: "bounced",
				},
			],
		});
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_REFUSED);
		expect(run(["PROC-daemon-0001", "s0001"])).toBe(EXIT_OK);
		expect(run(["PROC-fleet-0001", "s0001"])).toBe(EXIT_OK);
		expect(readProgress(root).runs["PROC-fleet-0001"]?.default?.["s0001"]?.how).toBe("found-done");
	});

	it("runs a command without recording progress", () => {
		write("skills/pij/SKILL.md", "# pij\n");
		procedure(
			"docs",
			"0001",
			"x",
			{ commands: [{ name: "serve", title: "serve it", run: "serve" }], steps: THREE_STEPS.steps },
			`${THREE_RECIPES}serve:\n    true\n`,
			"skills/pij/procedures",
		);
		expect(run(["PROC-docs-0001", "serve"])).toBe(EXIT_OK);
		expect(runsOf("serve")).toHaveLength(1);
		expect(readProgress(root).runs["PROC-docs-0001"]).toBeUndefined();
	});
});

describe("new", () => {
	it("books the number, writes a valid folder in the worktree and stages it", () => {
		claimNumber = 3;
		expect(run(["new", "fleet", "change-effort", "--title", "Change a seat's effort"])).toBe(
			EXIT_OK,
		);
		const doc = JSON.parse(
			readFileSync(
				join(root, "docs/project/procedures/fleet/PROC-fleet-0003-change-effort/procedure.dd.json"),
				"utf8",
			),
		);
		expect(doc.dd.schema).toBe("pij/procedure");
		expect(doc.sections[0].value).toMatchObject({
			id: "PROC-fleet-0003",
			area: "fleet",
			status: "draft",
			owner: "pij-test",
		});
		expect(calls.find((c) => c.file === "pij")?.args).toEqual([
			"claim",
			"procedure-fleet",
			"--for",
			"change-effort",
		]);
		expect(calls.some((c) => c.file === "git" && c.args[0] === "add")).toBe(true);
		expect(out.join("\n")).toContain(`worktree: ${root} (branch feature)`);
	});

	it("writes a skill procedure into pij's repo before its procedures folder exists", () => {
		write("skills/pij/SKILL.md", "# pij\n");
		expect(
			run(["new", "meta", "write-one", "--skill", "--kind", "meta", "--title", "Write one"]),
		).toBe(EXIT_OK);
		const doc = readFileSync(
			join(root, "skills/pij/procedures/meta/PROC-meta-0001-write-one/procedure.dd.json"),
			"utf8",
		);
		expect(doc).toContain("PROC-meta-0001");
		expect(run(["new", "fleet", "x", "--skill", "--title", "t"])).toBe(EXIT_REFUSED);
	});

	it("refuses on main, in the main checkout, for a skill area, and below an existing number", () => {
		writeFileSync(join(root, "gitdir", "HEAD"), "ref: refs/heads/main\n");
		expect(run(["new", "fleet", "x", "--title", "t"])).toBe(EXIT_REFUSED);
		writeFileSync(join(root, "gitdir", "HEAD"), "ref: refs/heads/feature\n");
		rmSync(join(root, ".git"));
		mkdirSync(join(root, ".git"));
		writeFileSync(join(root, ".git", "HEAD"), "ref: refs/heads/feature\n");
		expect(run(["new", "fleet", "x", "--title", "t"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain("is the main checkout");
		rmSync(join(root, ".git"), { recursive: true });
		writeFileSync(join(root, ".git"), `gitdir: ${join(root, "gitdir")}\n`);
		write("skills/pij/SKILL.md", "# pij\n");
		expect(run(["new", "meta", "x", "--title", "t"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain(
			"area meta belongs to the pij skill's procedures here: add --skill",
		);
		rmSync(join(root, "skills"), { recursive: true });
		procedure("fleet", "0005", "old", THREE_STEPS, THREE_RECIPES);
		claimNumber = 2;
		expect(run(["new", "fleet", "x", "--title", "t"])).toBe(EXIT_REFUSED);
		expect(err.join("\n")).toContain("a folder already holds fleet 5");
		expect(calls.filter((c) => c.file === "pij")).toHaveLength(1);
	});
});

describe("status", () => {
	it("records the approver through ddocs for several procedures at once", () => {
		procedure("fleet", "0001", "a", THREE_STEPS, THREE_RECIPES);
		procedure("fleet", "0002", "b", THREE_STEPS, THREE_RECIPES);
		expect(run(["status", "current", "PROC-fleet-0001", "b"])).toBe(EXIT_USAGE);
		expect(run(["status", "current", "PROC-fleet-0001", "b", "--approved-by", "jordan"])).toBe(
			EXIT_USAGE,
		);
		expect(err.join("\n")).toContain("status needs --why");
		expect(
			run([
				"status",
				"current",
				"PROC-fleet-0001",
				"b",
				"--approved-by",
				"jordan",
				"--why",
				"'yes, both'",
			]),
		).toBe(EXIT_OK);
		const ddocs = calls.filter((c) => c.file === "ddocs");
		expect(
			ddocs.filter((c) => c.args[0] === "set" && c.args[1]?.endsWith("#meta/status")),
		).toHaveLength(2);
		const changelog = ddocs.find((c) => c.args[0] === "add");
		expect(JSON.parse(changelog?.args[2] ?? "{}")).toMatchObject({ by: "jordan" });
	});
});

describe("check", () => {
	it("passes a clean procedure", () => {
		procedure(
			"fleet",
			"0001",
			"x",
			{ ...THREE_STEPS, uses: [{ path: "harness/scripts/shared.sh", why: "shared" }] },
			`${THREE_RECIPES}x:\n    harness/scripts/shared.sh\n`,
		);
		write("harness/scripts/shared.sh", "true\n");
		expect(run(["check"])).toBe(EXIT_OK);
		expect(out.join("\n")).toContain("1 procedure(s) clean");
	});

	it("finds missing recipes, access steps without a done check, undeclared paths and relayed messages", () => {
		write("harness/scripts/only-me.sh", "true\n");
		const dir = procedure(
			"fleet",
			"0001",
			"x",
			{
				steps: [
					{ id: "1", title: "a", kind: "run", run: "nope", changes_access: "yes", done_when: "x" },
				],
			},
			"x:\n    harness/scripts/only-me.sh\n",
		);
		write(`${dir}/notes.md`, `${RELAYED_MESSAGE}pij-a"] a body\n`);
		expect(run(["check"])).toBe(EXIT_FAIL);
		const text = out.join("\n");
		expect(text).toContain("step id 1 is not s and four digits");
		expect(text).toContain(
			"names recipe nope, which is in neither its justfile nor the root justfile",
		);
		expect(text).toContain("changes access but has no done check");
		expect(text).toContain("uses harness/scripts/only-me.sh outside its folder");
		expect(text).toContain("holds a relayed pij message");
	});

	it("finds layout problems: a skill area in pij's project home and a folder that disagrees with its id", () => {
		write("skills/pij/SKILL.md", "# pij\n");
		procedure("meta", "0001", "x", THREE_STEPS, THREE_RECIPES);
		const dir = procedure(
			"fleet",
			"0002",
			"y",
			{ ...THREE_STEPS, meta: { id: "PROC-fleet-0009" } },
			THREE_RECIPES,
		);
		write("docs/project/procedures/fleet/stray.md", "x\n");
		expect(run(["check"])).toBe(EXIT_FAIL);
		const text = out.join("\n");
		expect(text).toContain("area meta belongs to the pij skill's procedures");
		expect(text).toContain(
			`${dir}/procedure.dd.json: meta.id is PROC-fleet-0009, its folder says PROC-fleet-0002`,
		);
		expect(text).toContain("stray.md: only PROC-<area>-NNNN-<slug> folders belong in an area");
	});

	it("flags a page of numbered commands outside a procedure folder unless it says why", () => {
		const page = "1. `just a`\n2. `just b`\n3. `just c`\n";
		write("docs/how/steps.md", page);
		expect(run(["check"])).toBe(EXIT_FAIL);
		expect(out.join("\n")).toContain("docs/how/steps.md: 3 numbered steps begin with a command");
		write("docs/how/steps.md", `<!-- not-a-procedure: a reference list of verbs -->\n${page}`);
		out = [];
		expect(run(["check"])).toBe(EXIT_OK);
	});
});

describe("adopting in another project", () => {
	it("lets a project's own procedure hide the skill's with the same id, and book skill areas", () => {
		procedure("meta", "0001", "skill-one", THREE_STEPS, THREE_RECIPES, "skillhome");
		procedure("meta", "0002", "skill-two", THREE_STEPS, THREE_RECIPES, "skillhome");
		procedure("meta", "0001", "ours", THREE_STEPS, THREE_RECIPES);
		const listing = (): string[] => {
			out = [];
			main(["list", "--json"], io(), { skillHome: join(root, "skillhome") });
			return JSON.parse(out.join("\n")).procedures.map(
				(p: { id: string; path: string }) => `${p.id} ${p.path}`,
			);
		};
		expect(listing()).toEqual([
			"PROC-meta-0001 docs/project/procedures/meta/PROC-meta-0001-ours",
			"PROC-meta-0002 skillhome/meta/PROC-meta-0002-skill-two",
		]);
		claimNumber = 6;
		expect(
			main(["new", "meta", "mine", "--title", "t"], io(), { skillHome: join(root, "skillhome") }),
		).toBe(EXIT_OK);
	});

	it("reads its procedures home and its legacy pages from .pij-procedures.json; a legacy page may only shrink", () => {
		write(
			".pij-procedures.json",
			JSON.stringify({ home: "docs/procedures", legacy_pages: { "docs/procedures/old.md": 3 } }),
		);
		procedure("env", "0001", "x", THREE_STEPS, THREE_RECIPES, "docs/procedures");
		write("docs/procedures/old.md", "1. `just a`\n2. `just b`\n3. `just c`\n");
		expect(run(["check"])).toBe(EXIT_OK);
		expect(out.join("\n")).toContain("1 procedure(s) clean");
		write("docs/procedures/old.md", "1. `just a`\n2. `just b`\n3. `just c`\n4. `just d`\n");
		out = [];
		expect(run(["check"])).toBe(EXIT_FAIL);
		expect(out.join("\n")).toContain(
			"docs/procedures/old.md: a legacy page in .pij-procedures.json grew from 3 to 4",
		);
	});
});

describe("progress file", () => {
	it("is shared by every worktree of the clone, so a decision survives its worktree", () => {
		procedure("fleet", "0001", "x", {
			steps: [
				{ id: "s0001", title: "approve", kind: "decision", decide: "operator", done_when: "yes" },
			],
		});
		const common = join(root, "gitdir");
		const worktreeGit = join(common, "worktrees", "other");
		mkdirSync(worktreeGit, { recursive: true });
		writeFileSync(join(worktreeGit, "HEAD"), "ref: refs/heads/other\n");
		writeFileSync(join(worktreeGit, "commondir"), "../..\n");
		const other = mkdtempSync(join(tmpdir(), "pij-proc-wt-"));
		writeFileSync(join(other, ".git"), `gitdir: ${worktreeGit}\n`);
		const saved = root;
		root = other;
		procedure("fleet", "0001", "x", {
			steps: [
				{ id: "s0001", title: "approve", kind: "decision", decide: "operator", done_when: "yes" },
			],
		});
		root = saved;
		expect(run(["PROC-fleet-0001", "s0001", "--decided-by", "jordan", "--note", "yes"])).toBe(
			EXIT_OK,
		);
		expect(progressDir(other)).toBe(progressDir(root));
		out = [];
		expect(run(["PROC-fleet-0001", "s0001"], { cwd: other })).toBe(EXIT_OK);
		expect(out.join("\n")).toContain("already decided by jordan");
		rmSync(other, { recursive: true, force: true });
	});

	it("lives in the clone's git directory, whatever the working directory", () => {
		const dir = procedure("fleet", "0001", "x", THREE_STEPS, THREE_RECIPES);
		expect(run(["PROC-fleet-0001", "s0002"], { cwd: join(root, dir) })).toBe(EXIT_OK);
		expect(
			JSON.parse(readFileSync(runFile(root, "PROC-fleet-0001", "default"), "utf8")).steps["s0002"]
				.how,
		).toBe("ran");
		expect(runFile(root, "PROC-fleet-0001", "default")).toBe(
			join(root, "gitdir", "pij-procedures", "runs", "PROC-fleet-0001", "default.json"),
		);
	});
});

describe("real io", () => {
	it("stops a check that outlives its timeout and reports it as timed out", () => {
		const result = realIo().exec("/bin/sleep", ["5"], root, {}, false, 100);
		expect(result.status).toBe(EXIT_TIMED_OUT);
		expect(result.stderr).toContain("timed out");
	});
});

describe("a pod template's procedures (PRD 010 req-01)", () => {
	function template(folder: string, procedures: unknown[]): void {
		write(
			`${folder}/pod-template.dd.json`,
			JSON.stringify({
				dd: { schema: "pij/pod-template" },
				sections: [
					{ name: "meta", value: { name: "build", title: "build", version: "1" } },
					{ name: "slots", value: [{ id: "lead", role: "lead" }] },
					{ name: "procedures", value: procedures },
				],
			}),
		);
	}

	it("runs its own procedures and the repo's it links by id, and lists a linked one this repo lacks as missing", () => {
		template("pods/build", [
			{ id: "PROC-pod-0001", from: "template", use: "close" },
			{ id: "PROC-fleet-0002", from: "repo" },
			{ id: "PROC-fleet-0009", from: "repo" },
		]);
		procedure("pod", "0001", "close-run", THREE_STEPS, THREE_RECIPES, "pods/build/procedures");
		procedure("fleet", "0002", "branch-and-worktree", THREE_STEPS, THREE_RECIPES);
		// A repo procedure with the template's own id is hidden by the template's.
		procedure("pod", "0001", "repo-copy", THREE_STEPS, THREE_RECIPES);
		expect(run(["list", "--template", "pods/build"])).toBe(EXIT_OK);
		const listed = out.join("\n");
		expect(listed).toContain(
			"PROC-pod-0001 [close]  close-run title  (its own: pods/build/procedures/pod/PROC-pod-0001-close-run)",
		);
		expect(listed).toContain("PROC-fleet-0002  branch-and-worktree title  (linked from the repo");
		expect(listed).toContain("PROC-fleet-0009  MISSING (linked from the repo)");
		out = [];
		expect(run(["PROC-pod-0001", "--template", "pods/build"])).toBe(EXIT_OK);
		expect(out.join("\n")).toContain(
			"from: the pod template's own, pods/build/procedures/pod/PROC-pod-0001-close-run",
		);
		expect(run(["PROC-pod-0001", "s0002", "--template", "pods/build"])).toBe(EXIT_OK);
		expect(run(["PROC-fleet-0002", "s0002", "--template", "pods/build"])).toBe(EXIT_OK);
		expect(run(["check", "--template", "pods/build"])).toBe(EXIT_USAGE);
	});

	it("pod down runs the close procedure first, and closes nothing when it stops early", () => {
		template("pods/build", [{ id: "PROC-pod-0001", from: "template", use: "close" }]);
		procedure(
			"pod",
			"0001",
			"close-run",
			{
				steps: [
					{ id: "s0001", title: "push the branch", kind: "run", run: "one", done_when: "pushed" },
					{
						id: "s0002",
						title: "merge it",
						kind: "decision",
						decide: "the prime",
						done_when: "merged",
					},
				],
			},
			THREE_RECIPES,
			"pods/build/procedures",
		);
		podShown = {
			close_procedure: { id: "PROC-pod-0001", template: join(root, "pods/build"), key: "pod-b1" },
		};
		const downs = () => calls.filter((c) => c.file === "pij-rs" && c.args.includes("down"));
		const args = ["pod", "down", "b1"];
		const options = { skillHome: join(root, "no-skill-home") };
		expect(podDown(args, args.slice(1), io(), "/bin/pij-rs", options)).toBe(EXIT_REFUSED);
		expect(downs()).toEqual([]);
		expect(runsOf("one")).toHaveLength(1);
		expect(err.join("\n")).toContain(
			"close procedure PROC-pod-0001 stopped at step s0002; nothing was closed",
		);
		expect(err.join("\n")).toContain(
			`resume: just proc PROC-pod-0001 s0002 --template ${join(root, "pods/build")} --key pod-b1`,
		);

		expect(
			run([
				"PROC-pod-0001",
				"s0002",
				"--template",
				"pods/build",
				"--key",
				"pod-b1",
				"--decided-by",
				"prime",
				"--note",
				"merged",
			]),
		).toBe(EXIT_OK);
		expect(podDown(args, args.slice(1), io(), "/bin/pij-rs", options)).toBe(EXIT_OK);
		expect(downs().map((c) => c.args)).toEqual([["pod", "down", "b1", "--close-procedure-ran"]]);

		const skip = ["pod", "down", "b1", "--skip-close-procedure"];
		calls = [];
		expect(podDown(skip, skip.slice(1), io(), "/bin/pij-rs", options)).toBe(EXIT_OK);
		expect(calls.map((c) => c.args)).toEqual([skip]);
	});

	it("pod down goes straight to pij-rs when the template names no close procedure", () => {
		const args = ["pod", "down", "b1"];
		expect(podDown(args, args.slice(1), io(), "/bin/pij-rs")).toBe(EXIT_OK);
		expect(calls.map((c) => c.args)).toEqual([["--json", "pod", "show", "b1"], args]);
	});
});
