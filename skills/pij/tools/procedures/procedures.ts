/**
 * The pij procedure runner (PRD 005, blueprint 004).
 *
 * A procedure is a folder, `<home>/<area>/PROC-<area>-NNNN-<slug>/`, holding `procedure.dd.json` (schema
 * `pij/procedure`), its rendering and anything only it uses, including its own justfile. This one file runs steps,
 * records progress per checkout, books new procedures, lists them, records status changes and holds the drift check.
 *
 *   just proc …            in pij (the root justfile's `proc` recipe)
 *   pij proc …             anywhere pij is installed (the shim loads this file in-process)
 *
 * Cost (blueprint 004 k-1..k-5): nothing runs in the background. Listing reads files. A step launches one `just` per
 * check and one for its recipe. The check launches `git ls-files` once and `just --summary` once per justfile.
 *
 * Runs on node's type stripping with no dependencies, so it works in any repo. Side effects go through `Io`.
 */
import { spawnSync } from "node:child_process";
import {
	accessSync,
	constants,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	realpathSync,
	renameSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------------------------------------------
// Constants

/** A project's own procedures, relative to its repo root. */
export const PROJECT_HOME = "docs/project/procedures";
/** Procedures that ship with the pij skill, relative to pij's repo root. */
export const SKILL_HOME = "skills/pij/procedures";
/** Areas that live in the skill's home; a project's own home refuses them so ids never collide. */
export const SKILL_AREAS: readonly string[] = ["meta", "docs", "pods"];
export const DOC_FILE = "procedure.dd.json";
export const SCHEMA = "pij/procedure";
/**
 * Run progress and decisions, in the clone's shared git directory, so every worktree of the clone sees them and
 * removing a worktree loses nothing. Outside git (never in practice) it falls back to `.pij/` under the root.
 * One file per run, `<dir>/<id>/<key>.json`, so runs of different keys at once never overwrite each other.
 */
export const PROGRESS_DIR = "pij-procedures/runs";
export const PROGRESS_FALLBACK = ".pij/procedures/runs";
/** A run key names a file, so it is a plain word and never a path. `--key` also refuses `Object.prototype` names. */
export const KEY_RE = /^[A-Za-z0-9][A-Za-z0-9._@-]*$/;
/** A precondition or done check that runs longer than this counts as failing (a hung check never hangs the run). */
export const CHECK_TIMEOUT_MS = 300_000;
/** The status a timed-out check reports, as `timeout(1)` does. */
export const EXIT_TIMED_OUT = 124;
/** A repo's optional settings for procedures (committed): `{"home": "<path>", "legacy_pages": {"<page>": <count>}}`. */
export const CONFIG_FILE = ".pij-procedures.json";
export const DEFAULT_KEY = "default";
export const AREA_RE = /^[a-z][a-z0-9]*$/;
export const SLUG_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
export const FOLDER_RE = /^PROC-([a-z][a-z0-9]*)-(\d{4})-([a-z0-9]+(?:-[a-z0-9]+)*)$/;
export const ID_RE = /^PROC-([a-z][a-z0-9]*)-(\d{4})$/;
/** A step id: `s` and four digits (ddocs address segments start with a letter). The CLI also takes the digits alone. */
export const STEP_ID_RE = /^s\d{4}$/;
export const STATUSES: readonly string[] = ["draft", "current", "superseded", "retired"];
export const KINDS: readonly string[] = ["runnable", "mixed", "guide", "meta"];
/** A page that numbers commands but is reference, not a procedure, says why (PRD 005 req-31, q-09). */
export const NOT_A_PROCEDURE_RE = /<!--\s*not-a-procedure:\s*([^>]*?\S)\s*-->/;
/** Pages scanned for numbered commands outside a procedure folder (req-31). */
export const NUMBERED_SCAN_ROOTS: readonly string[] = [
	"docs/how",
	"docs/project",
	"skills/pij/references/community",
];
/** Three or more numbered items that each begin with a command make a page a procedure (q-09). */
export const NUMBERED_COMMAND_LIMIT = 3;
/**
 * The head pij puts on a relayed message; never stored in a procedure folder (req-27). Built from parts so this
 * detector doesn't itself trip the guard that keeps legacy frame heads out of maintained code (core/message.test.ts).
 */
export const RELAYED_MESSAGE = `${"[pij"} from "`;
/** Blueprint component kinds the report expects a procedure to operate. */
export const OPERATED_KINDS: readonly string[] = ["verb", "endpoint", "hook"];
/** Files in a procedure folder scanned for the paths they use (req-05). */
const SCRIPT_RE = /(^justfile$|\.(sh|bash|ts|mjs|cjs|js|py)$)/;

export const EXIT_OK = 0;
export const EXIT_FAIL = 1;
export const EXIT_USAGE = 2;
export const EXIT_REFUSED = 3;

// ---------------------------------------------------------------------------------------------------------------
// Types

export interface Input {
	name: string;
	description: string;
	required: boolean;
	fallback?: string;
	secret: boolean;
}

export interface Check {
	check: string;
	fix: string;
}

export interface Precondition extends Check {
	id: string;
	text: string;
}

export type StepKind = "run" | "decision" | "guide";

export interface Step {
	id: string;
	title: string;
	kind: StepKind;
	run?: string;
	decide?: string;
	follow?: string;
	inputs: string[];
	pre: Check[];
	requires: string[];
	done?: string;
	doneWhen: string;
	who?: string;
	changesAccess: boolean;
}

export interface Command {
	name: string;
	title: string;
	run: string;
}

export interface PathNote {
	path: string;
	why: string;
}

/** `template`: a pod template's own procedures, under `procedures/` in its folder (PRD 010 req-01). */
export type HomeKind = "project" | "skill" | "template";

export interface Home {
	kind: HomeKind;
	dir: string;
}

export interface Procedure {
	id: string;
	title: string;
	area: string;
	kind: string;
	status: string;
	owner: string;
	supersededBy?: string;
	who: string;
	when: string;
	inputs: Input[];
	preconditions: Precondition[];
	steps: Step[];
	commands: Command[];
	doneWhen: string;
	uses: PathNote[];
	sideEffects: PathNote[];
	operates: string[];
	/** Absolute folder and document paths. */
	dir: string;
	file: string;
	home: Home;
}

export interface ExecResult {
	status: number;
	stdout: string;
	stderr: string;
}

/** Every side effect the runner has, injected (P3). */
export interface Io {
	cwd: string;
	env: Record<string, string | undefined>;
	out: (line: string) => void;
	err: (line: string) => void;
	/** Start `file` directly (no shell). `inherit` passes the terminal through and captures nothing. */
	exec: (
		file: string,
		args: string[],
		cwd: string,
		env: Record<string, string | undefined>,
		inherit: boolean,
		/** Stop the child after this long; it then reports EXIT_TIMED_OUT. */
		timeoutMs?: number,
	) => ExecResult;
	now: () => Date;
}

export interface StepRecord {
	at: string;
	by: string;
	how: "ran" | "found-done" | "decided" | "marked";
	note?: string;
}

export interface Progress {
	version: 1;
	runs: Record<string, Record<string, Record<string, StepRecord>>>;
}

// ---------------------------------------------------------------------------------------------------------------
// Small helpers

type Json = Record<string, unknown>;

function isObj(value: unknown): value is Json {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The schema's yes/no fields (ddocs has no boolean type); `true` and `false` are read the same way. */
function isYes(value: unknown): boolean {
	return value === "yes" || value === true;
}

function isNo(value: unknown): boolean {
	return value === "no" || value === false;
}

function str(value: unknown): string | undefined {
	return typeof value === "string" ? value : undefined;
}

function list(value: unknown): unknown[] {
	return Array.isArray(value) ? value : [];
}

function isFile(path: string): boolean {
	try {
		return statSync(path).isFile();
	} catch {
		return false;
	}
}

function isDir(path: string): boolean {
	try {
		return statSync(path).isDirectory();
	} catch {
		return false;
	}
}

function readText(path: string): string | undefined {
	try {
		return readFileSync(path, "utf8");
	} catch {
		return undefined;
	}
}

/** `yyyy-mm-dd` in local time. */
export function localDate(date: Date): string {
	const pad = (n: number) => String(n).padStart(2, "0");
	return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** Shell-style words: whitespace splits, single and double quotes group. No expansion. */
export function splitWords(line: string): string[] {
	const words: string[] = [];
	let word = "";
	let quote: string | undefined;
	let started = false;
	for (const ch of line) {
		if (quote) {
			if (ch === quote) quote = undefined;
			else word += ch;
		} else if (ch === "'" || ch === '"') {
			quote = ch;
			started = true;
		} else if (/\s/.test(ch)) {
			if (started) words.push(word);
			word = "";
			started = false;
		} else {
			word += ch;
			started = true;
		}
	}
	if (started) words.push(word);
	return words;
}

/** The absolute path of `name` on PATH, found in-process (no launch). */
export function findOnPath(
	name: string,
	env: Record<string, string | undefined>,
): string | undefined {
	for (const dir of (env.PATH ?? "").split(":")) {
		if (!dir) continue;
		const candidate = join(dir, name);
		try {
			accessSync(candidate, constants.X_OK);
			if (isFile(candidate)) return candidate;
		} catch {
			// not here
		}
	}
	return undefined;
}

/** The recipe names a justfile defines, read in-process. */
export function recipesIn(text: string): Set<string> {
	const names = new Set<string>();
	for (const line of text.split("\n")) {
		if (!line || /^[\s#[]/.test(line)) continue;
		if (/^(set|export|import|mod)\s/.test(line)) continue;
		const alias = /^alias\s+([A-Za-z_][A-Za-z0-9_-]*)\s*:=/.exec(line);
		if (alias?.[1]) {
			names.add(alias[1]);
			continue;
		}
		const recipe = /^@?([A-Za-z_][A-Za-z0-9_-]*)(?:\s+[^:]*)?:(?!=)/.exec(line);
		if (recipe?.[1]) names.add(recipe[1]);
	}
	return names;
}

/** The environment variable that carries input `name` to a recipe. */
export function inputVar(name: string): string {
	return `PROC_IN_${name.toUpperCase().replace(/[^A-Z0-9]/g, "_")}`;
}

/** The repo root: the nearest directory at or above `start` holding `.git` (in-process, no git launch). */
export function findRoot(start: string): string | undefined {
	let dir = resolve(start);
	for (;;) {
		if (existsSync(join(dir, ".git"))) return dir;
		const parent = dirname(dir);
		if (parent === dir) return undefined;
		dir = parent;
	}
}

/** The checked-out branch and whether `root` is the main checkout (its `.git` is a directory, not a worktree link). */
export function gitState(root: string): { branch?: string; mainCheckout: boolean } {
	const dotGit = join(root, ".git");
	const mainCheckout = isDir(dotGit);
	let gitDir = dotGit;
	if (!mainCheckout) {
		const pointer = /^gitdir:\s*(.+)$/m.exec(readText(dotGit) ?? "");
		if (!pointer?.[1]) return { mainCheckout };
		gitDir = resolve(root, pointer[1].trim());
	}
	const head = /^ref:\s*refs\/heads\/(.+)$/m.exec(readText(join(gitDir, "HEAD")) ?? "");
	return { branch: head?.[1]?.trim(), mainCheckout };
}

// ---------------------------------------------------------------------------------------------------------------
// Reading procedures

function sectionMap(doc: unknown): Map<string, unknown> {
	const map = new Map<string, unknown>();
	if (!isObj(doc)) return map;
	for (const section of list(doc.sections)) {
		if (isObj(section) && typeof section.name === "string") map.set(section.name, section.value);
	}
	return map;
}

function pathNotes(value: unknown): PathNote[] {
	return list(value)
		.filter(isObj)
		.map((v) => ({ path: str(v.path) ?? "", why: str(v.why) ?? "" }))
		.filter((v) => v.path !== "");
}

function objectOf(value: unknown): Json {
	return isObj(value) ? value : {};
}

/** A procedure from its parsed document; `undefined` when it is not a `pij/procedure` document. */
export function parseProcedure(doc: unknown, file: string, home: Home): Procedure | undefined {
	if (!isObj(doc) || !isObj(doc.dd) || doc.dd.schema !== SCHEMA) return undefined;
	const s = sectionMap(doc);
	const meta = objectOf(s.get("meta"));
	const whoWhen = objectOf(s.get("who_when"));
	const steps: Step[] = list(s.get("steps"))
		.filter(isObj)
		.map((v) => {
			const kind = str(v.kind);
			return {
				id: str(v.id) ?? "",
				title: str(v.title) ?? "",
				kind: kind === "decision" || kind === "guide" ? kind : "run",
				run: str(v.run),
				decide: str(v.decide),
				follow: str(v.follow),
				inputs: list(v.inputs).filter((x): x is string => typeof x === "string"),
				pre: list(v.pre)
					.filter(isObj)
					.map((p) => ({ check: str(p.check) ?? "", fix: str(p.fix) ?? "" })),
				requires: list(v.requires).filter((x): x is string => typeof x === "string"),
				done: str(v.done),
				doneWhen: str(v.done_when) ?? "",
				who: str(v.who),
				changesAccess: isYes(v.changes_access),
			};
		});
	return {
		id: str(meta.id) ?? "",
		title: str(meta.title) ?? "",
		area: str(meta.area) ?? "",
		kind: str(meta.kind) ?? "",
		status: str(meta.status) ?? "",
		owner: str(meta.owner) ?? "",
		supersededBy: str(meta.superseded_by),
		who: str(whoWhen.who) ?? "",
		when: str(whoWhen.when) ?? "",
		inputs: list(s.get("inputs"))
			.filter(isObj)
			.map((v) => ({
				name: str(v.name) ?? "",
				description: str(v.description) ?? "",
				required: !isNo(v.required),
				fallback: str(v.fallback),
				secret: isYes(v.secret),
			})),
		preconditions: list(s.get("preconditions"))
			.filter(isObj)
			.map((v) => ({
				id: str(v.id) ?? "",
				check: str(v.check) ?? "",
				text: str(v.text) ?? "",
				fix: str(v.fix) ?? "",
			})),
		steps,
		commands: list(s.get("commands"))
			.filter(isObj)
			.map((v) => ({ name: str(v.name) ?? "", title: str(v.title) ?? "", run: str(v.run) ?? "" })),
		doneWhen: str(s.get("done_when")) ?? "",
		uses: pathNotes(s.get("uses")),
		sideEffects: pathNotes(s.get("side_effects")),
		operates: list(s.get("operates"))
			.filter(isObj)
			.map((v) => str(v.component) ?? "")
			.filter((v) => v !== ""),
		dir: dirname(file),
		file,
		home,
	};
}

/** Whether `root` is pij's own repo, the one that holds the skill (and so the skill's procedures). */
export function isPijRepo(root: string): boolean {
	return isFile(join(root, "skills", "pij", "SKILL.md"));
}

export interface Config {
	/** The project's procedures home, relative to the root (default docs/project/procedures). */
	home: string;
	/** Pages from before procedures, with the numbered commands each held: they may shrink, never grow (req-31). */
	legacyPages: Map<string, number>;
}

/** The repo's `.pij-procedures.json`, or the defaults. */
export function readConfig(root: string): Config {
	const config: Config = { home: PROJECT_HOME, legacyPages: new Map() };
	const text = readText(join(root, CONFIG_FILE));
	if (text === undefined) return config;
	let parsed: unknown;
	try {
		parsed = JSON.parse(text);
	} catch {
		return config;
	}
	if (!isObj(parsed)) return config;
	const home = str(parsed.home);
	if (home && !home.startsWith("/") && !home.split("/").includes(".."))
		config.home = home.replace(/\/$/, "");
	if (isObj(parsed.legacy_pages)) {
		for (const [page, count] of Object.entries(parsed.legacy_pages)) {
			if (typeof count === "number") config.legacyPages.set(page, count);
		}
	}
	return config;
}

/** The homes for a checkout: its own project home, and the skill's (in pij's repo, else beside this runner). */
export function homesFor(
	root: string,
	skillHome: string,
	config: Config = readConfig(root),
): Home[] {
	return [
		{ kind: "project", dir: join(root, config.home) },
		{ kind: "skill", dir: isPijRepo(root) ? join(root, SKILL_HOME) : skillHome },
	];
}

export interface Loaded {
	procedures: Procedure[];
	/** Layout problems, for the check (req-03). */
	problems: string[];
}

/** Every procedure in `homes`, read from the files, with any layout problems found on the way. */
export function loadProcedures(
	homes: Home[],
	root: string,
	legacy: ReadonlySet<string> = new Set(),
): Loaded {
	const procedures: Procedure[] = [];
	const problems: string[] = [];
	const areasByHome = new Map<HomeKind, Set<string>>();
	const where = (path: string) => (path.startsWith(root + sep) ? relative(root, path) : path);
	for (const home of homes) {
		const areas = new Set<string>();
		areasByHome.set(home.kind, areas);
		if (!isDir(home.dir)) continue;
		for (const area of readdirSync(home.dir).sort()) {
			const areaDir = join(home.dir, area);
			if (!isDir(areaDir)) {
				if (area !== "README.md" && !legacy.has(where(areaDir)))
					problems.push(`${where(areaDir)}: only area folders belong in ${where(home.dir)}`);
				continue;
			}
			if (!AREA_RE.test(area)) {
				problems.push(`${where(areaDir)}: an area is lowercase letters and digits`);
				continue;
			}
			areas.add(area);
			for (const name of readdirSync(areaDir).sort()) {
				const dir = join(areaDir, name);
				const match = FOLDER_RE.exec(name);
				if (!match || !isDir(dir)) {
					if (legacy.has(where(dir))) continue;
					problems.push(`${where(dir)}: only PROC-<area>-NNNN-<slug> folders belong in an area`);
					continue;
				}
				const file = join(dir, DOC_FILE);
				const text = readText(file);
				if (text === undefined) {
					problems.push(`${where(dir)}: has no ${DOC_FILE}`);
					continue;
				}
				let doc: unknown;
				try {
					doc = JSON.parse(text);
				} catch (error) {
					problems.push(`${where(file)}: not JSON (${(error as Error).message})`);
					continue;
				}
				const procedure = parseProcedure(doc, file, home);
				if (!procedure) {
					problems.push(`${where(file)}: its schema is not ${SCHEMA}`);
					continue;
				}
				const id = `PROC-${match[1]}-${match[2]}`;
				if (match[1] !== area) problems.push(`${where(dir)}: the folder's area is not ${area}`);
				if (procedure.id !== id)
					problems.push(
						`${where(file)}: meta.id is ${procedure.id || "missing"}, its folder says ${id}`,
					);
				if (procedure.area !== area)
					problems.push(`${where(file)}: meta.area is ${procedure.area || "missing"}, not ${area}`);
				procedures.push(procedure);
			}
		}
	}
	const project = areasByHome.get("project") ?? new Set<string>();
	if (!isPijRepo(root)) {
		// Another project's own procedures win: one with the same id as a skill procedure hides it (its ids never move).
		const own = new Set(procedures.filter((p) => p.home.kind === "project").map((p) => p.id));
		return {
			procedures: procedures.filter((p) => p.home.kind !== "skill" || !own.has(p.id)),
			problems,
		};
	}
	// In pij's own repo both homes are in one tree, so an area lives in one of them.
	const projectHome = homes.find((h) => h.kind === "project")?.dir ?? "";
	for (const area of areasByHome.get("skill") ?? []) {
		if (project.has(area))
			problems.push(
				`area ${area} is in both ${where(projectHome)} and the skill's home: an area lives in one`,
			);
	}
	for (const area of project) {
		if (SKILL_AREAS.includes(area))
			problems.push(
				`area ${area} belongs to the pij skill's procedures, not ${where(projectHome)}`,
			);
	}
	return { procedures, problems };
}

function basenameOf(path: string): string {
	return path.slice(path.lastIndexOf(sep) + 1);
}

/** A procedure by id (any case), folder name or slug; several when a slug is ambiguous. */
export function findProcedure(
	procedures: Procedure[],
	ref: string,
): Procedure | Procedure[] | undefined {
	const lower = ref.toLowerCase();
	const exact = procedures.find(
		(p) => p.id.toLowerCase() === lower || basenameOf(p.dir).toLowerCase() === lower,
	);
	if (exact) return exact;
	const bySlug = procedures.filter((p) => FOLDER_RE.exec(basenameOf(p.dir))?.[3] === lower);
	if (bySlug.length === 1) return bySlug[0];
	return bySlug.length > 1 ? bySlug : undefined;
}

/** Where a guide step's page is: `./` and `../` paths are relative to the procedure's folder, others to the repo root. */
export function followPath(root: string, procedure: Procedure, follow: string): string {
	return follow.startsWith("./") || follow.startsWith("../")
		? resolve(procedure.dir, follow)
		: join(root, follow);
}

// ---------------------------------------------------------------------------------------------------------------
// Progress (c-progress)

/** The git directory a checkout's worktrees share (`.git`, or a linked worktree's `commondir`), found in-process. */
export function commonGitDir(root: string): string | undefined {
	const dotGit = join(root, ".git");
	if (isDir(dotGit)) return dotGit;
	const pointer = /^gitdir:\s*(.+)$/m.exec(readText(dotGit) ?? "");
	if (!pointer?.[1]) return undefined;
	const gitDir = resolve(root, pointer[1].trim());
	const common = readText(join(gitDir, "commondir"))?.trim();
	return common ? resolve(gitDir, common) : gitDir;
}

/** The folder holding `root`'s run progress (shared by every worktree of the clone). */
export function progressDir(root: string): string {
	const common = commonGitDir(root);
	return common ? join(common, PROGRESS_DIR) : join(root, PROGRESS_FALLBACK);
}

/** One run's file. Ids and keys are checked before they get here (ID_RE, KEY_RE), so neither is a path. */
export function runFile(root: string, id: string, key: string): string {
	return join(progressDir(root), id, `${key}.json`);
}

function readRun(file: string): Record<string, StepRecord> | undefined {
	const text = readText(file);
	if (text === undefined) return undefined;
	try {
		const parsed: unknown = JSON.parse(text);
		if (isObj(parsed) && isObj(parsed.steps)) return parsed.steps as Record<string, StepRecord>;
	} catch {
		// a corrupt file reads as empty; the next write replaces it
	}
	return undefined;
}

export function readProgress(root: string): Progress {
	const progress: Progress = { version: 1, runs: {} };
	const dir = progressDir(root);
	if (!isDir(dir)) return progress;
	for (const id of readdirSync(dir)) {
		if (!ID_RE.test(id) || !isDir(join(dir, id))) continue;
		for (const name of readdirSync(join(dir, id))) {
			const key = name.endsWith(".json") ? name.slice(0, -".json".length) : "";
			if (!KEY_RE.test(key)) continue;
			const steps = readRun(join(dir, id, name));
			if (!steps) continue;
			progress.runs[id] ??= {};
			progress.runs[id][key] = steps;
		}
	}
	return progress;
}

function own<T>(record: Record<string, T> | undefined, name: string): T | undefined {
	return record && Object.hasOwn(record, name) ? record[name] : undefined;
}

export function recordOf(
	progress: Progress,
	id: string,
	key: string,
	step: string,
): StepRecord | undefined {
	return own(own(own(progress.runs, id), key), step);
}

/** Record one step in its run's own file (written whole, then renamed into place). */
export function saveRecord(
	root: string,
	id: string,
	key: string,
	step: string,
	record: StepRecord,
): void {
	const file = runFile(root, id, key);
	const steps = readRun(file) ?? {};
	steps[step] = record;
	mkdirSync(dirname(file), { recursive: true });
	const temporary = `${file}.${process.pid}.tmp`;
	writeFileSync(temporary, `${JSON.stringify({ version: 1, steps }, null, "\t")}\n`);
	renameSync(temporary, file);
}

export function clearRun(root: string, id: string, key: string): boolean {
	const file = runFile(root, id, key);
	if (!existsSync(file)) return false;
	rmSync(file);
	return true;
}

// ---------------------------------------------------------------------------------------------------------------
// Arguments

export interface Flags {
	positionals: string[];
	given: Set<string>;
	json: boolean;
	again: boolean;
	clear: boolean;
	done: boolean;
	skill: boolean;
	help: boolean;
	key?: string;
	args: Map<string, string>;
	decidedBy?: string;
	note?: string;
	title?: string;
	kind?: string;
	approvedBy?: string;
	why?: string;
	supersededBy?: string;
	owner?: string;
	template?: string;
}

const BOOL_FLAGS = new Set(["--json", "--again", "--clear", "--done", "--skill", "--help"]);
const VALUE_FLAGS = new Set([
	"--key",
	"--arg",
	"--decided-by",
	"--note",
	"--title",
	"--kind",
	"--approved-by",
	"--why",
	"--superseded-by",
	"--owner",
	"--template",
]);

/** Every flag is known or refused: nothing is swallowed (req-15). */
export function parseFlags(
	argv: string[],
): { ok: true; flags: Flags } | { ok: false; error: string } {
	const flags: Flags = {
		positionals: [],
		given: new Set(),
		json: false,
		again: false,
		clear: false,
		done: false,
		skill: false,
		help: false,
		args: new Map(),
	};
	for (let index = 0; index < argv.length; index++) {
		const token = argv[index] ?? "";
		if (token === "--") {
			flags.positionals.push(...argv.slice(index + 1));
			break;
		}
		if (token === "-h") {
			flags.help = true;
			flags.given.add("--help");
			continue;
		}
		if (!token.startsWith("--")) {
			flags.positionals.push(token);
			continue;
		}
		const equals = token.indexOf("=");
		const name = equals === -1 ? token : token.slice(0, equals);
		if (BOOL_FLAGS.has(name)) {
			if (equals !== -1) return { ok: false, error: `${name} takes no value` };
			flags.given.add(name);
			if (name === "--json") flags.json = true;
			else if (name === "--again") flags.again = true;
			else if (name === "--clear") flags.clear = true;
			else if (name === "--done") flags.done = true;
			else if (name === "--skill") flags.skill = true;
			else flags.help = true;
			continue;
		}
		if (!VALUE_FLAGS.has(name))
			return { ok: false, error: `unknown flag ${name} (see: proc help)` };
		let value: string | undefined;
		if (equals !== -1) value = token.slice(equals + 1);
		else {
			value = argv[index + 1];
			index++;
		}
		if (value === undefined) return { ok: false, error: `${name} needs a value` };
		flags.given.add(name);
		if (name === "--arg") {
			const split = value.indexOf("=");
			if (split <= 0) return { ok: false, error: `--arg takes name=value, not ${value}` };
			flags.args.set(value.slice(0, split), value.slice(split + 1));
		} else if (name === "--key") {
			if (!KEY_RE.test(value) || value in Object.prototype)
				return {
					ok: false,
					error: `--key takes letters, digits and . _ @ - (starting with a letter or digit), not ${JSON.stringify(value)}`,
				};
			flags.key = value;
		} else if (name === "--decided-by") flags.decidedBy = value;
		else if (name === "--note") flags.note = value;
		else if (name === "--title") flags.title = value;
		else if (name === "--kind") flags.kind = value;
		else if (name === "--approved-by") flags.approvedBy = value;
		else if (name === "--why") flags.why = value;
		else if (name === "--superseded-by") flags.supersededBy = value;
		else if (name === "--template") flags.template = value;
		else flags.owner = value;
	}
	return { ok: true, flags };
}

function onlyFlags(flags: Flags, allowed: string[], what: string): string | undefined {
	for (const flag of flags.given) {
		if (flag === "--help" || flag === "--template") continue;
		if (!allowed.includes(flag)) return `${flag} doesn't apply to ${what}`;
	}
	return undefined;
}

// ---------------------------------------------------------------------------------------------------------------
// The runner's context

export interface Options {
	/** Where the skill's procedures are when the repo has none of its own (default: beside this file). */
	skillHome?: string;
	/** A pod template's folder or file (`--template`): its own procedures come first (PRD 010 req-01). */
	template?: string;
}

/** The pod template file inside its folder (PRD 008). */
export const TEMPLATE_FILE = "pod-template.dd.json";
export const TEMPLATE_SCHEMA = "pij/pod-template";

/** A procedure a pod template names: its own (`template`) or linked from the repo (`repo`). */
export interface TemplateProcedure {
	id: string;
	from: "template" | "repo";
	use: string;
}

/** A pod template as the runner reads it. */
export interface TemplateInfo {
	/** Absolute folder. */
	folder: string;
	name: string;
	procedures: TemplateProcedure[];
}

/** Read the pod template at `path` (a folder holding pod-template.dd.json, or that file), or say why not. */
export function readTemplate(path: string): TemplateInfo | string {
	const file = isDir(path) ? join(path, TEMPLATE_FILE) : path;
	const text = readText(file);
	if (text === undefined)
		return `no pod template at ${path} (a folder holding ${TEMPLATE_FILE}, or that file)`;
	let doc: unknown;
	try {
		doc = JSON.parse(text);
	} catch (error) {
		return `${file}: not JSON (${(error as Error).message})`;
	}
	if (!isObj(doc) || !isObj(doc.dd) || doc.dd.schema !== TEMPLATE_SCHEMA)
		return `${file}: its schema is not ${TEMPLATE_SCHEMA}`;
	const sectionOf = (name: string) =>
		list(doc.sections).find((section) => isObj(section) && section.name === name) as
			| Json
			| undefined;
	const meta = sectionOf("meta")?.value;
	const procedures: TemplateProcedure[] = [];
	for (const row of list(sectionOf("procedures")?.value)) {
		if (!isObj(row)) continue;
		const id = str(row.id) ?? "";
		const from = row.from === "template" ? "template" : row.from === "repo" ? "repo" : undefined;
		if (!ID_RE.test(id) || !from) continue;
		procedures.push({ id, from, use: str(row.use) ?? "" });
	}
	return {
		folder: dirname(resolve(file)),
		name: (isObj(meta) ? str(meta.name) : undefined) ?? "",
		procedures,
	};
}

export interface Ctx {
	io: Io;
	root: string;
	homes: Home[];
	config: Config;
	loaded: Loaded;
	/** How to call the runner here: `just proc` where the root justfile has it, else `pij proc`. */
	entry: string;
	recipes: Map<string, Set<string>>;
	/** The pod template `--template` named, when it did. */
	template?: TemplateInfo;
}

const RUNNER_FILE = fileURLToPath(import.meta.url);
const DEFAULT_SKILL_HOME = resolve(dirname(RUNNER_FILE), "..", "..", "procedures");

function rootJustfile(root: string): string | undefined {
	for (const name of ["justfile", "Justfile", ".justfile"]) {
		if (isFile(join(root, name))) return join(root, name);
	}
	return undefined;
}

function recipesOf(ctx: Ctx, file: string): Set<string> {
	let names = ctx.recipes.get(file);
	if (!names) {
		names = recipesIn(readText(file) ?? "");
		ctx.recipes.set(file, names);
	}
	return names;
}

/** The justfile that defines `recipe` for `procedure`: its own first, then the repo's root justfile. */
function justfileFor(ctx: Ctx, procedure: Procedure, recipe: string): string | undefined {
	const own = join(procedure.dir, "justfile");
	if (isFile(own) && recipesOf(ctx, own).has(recipe)) return own;
	const root = rootJustfile(ctx.root);
	if (root && recipesOf(ctx, root).has(recipe)) return root;
	return undefined;
}

/** The runner's view of the checkout around `io.cwd`, or why there is none. */
export function context(io: Io, options: Options = {}): Ctx | string {
	const root = findRoot(io.cwd);
	if (!root) return `not inside a git checkout (${io.cwd})`;
	const config = readConfig(root);
	const homes = homesFor(root, options.skillHome ?? DEFAULT_SKILL_HOME, config);
	let template: TemplateInfo | undefined;
	if (options.template !== undefined) {
		const read = readTemplate(resolve(io.cwd, options.template));
		if (typeof read === "string") return read;
		template = read;
		homes.unshift({ kind: "template", dir: join(read.folder, "procedures") });
	}
	const rootFile = rootJustfile(root);
	const entry =
		rootFile && recipesIn(readText(rootFile) ?? "").has("proc") ? "just proc" : "pij proc";
	const loaded = loadProcedures(homes, root, new Set(config.legacyPages.keys()));
	if (template) {
		// A template's own procedure is the one its pod means: it hides a repo procedure with the same id.
		const own = new Set(
			loaded.procedures.filter((p) => p.home.kind === "template").map((p) => p.id),
		);
		loaded.procedures = loaded.procedures.filter(
			(p) => p.home.kind === "template" || !own.has(p.id),
		);
	}
	return {
		io,
		root,
		homes,
		config,
		loaded,
		entry,
		recipes: new Map(),
		template,
	};
}

function rel(ctx: Ctx, path: string): string {
	return path.startsWith(ctx.root + sep) ? relative(ctx.root, path) : path;
}

function by(io: Io): string {
	return io.env.PIJ_SESSION_ID ?? io.env.USER ?? "unknown";
}

// ---------------------------------------------------------------------------------------------------------------
// Running recipes

interface RecipeRun extends ExecResult {
	/** The recipe no justfile defines, or why `just` could not start. */
	missing?: string;
}

function runRecipe(
	ctx: Ctx,
	procedure: Procedure,
	command: string,
	env: Record<string, string | undefined>,
	inherit: boolean,
): RecipeRun {
	const [recipe, ...args] = splitWords(command);
	if (!recipe) return { status: 127, stdout: "", stderr: "", missing: "recipe (empty command)" };
	const file = justfileFor(ctx, procedure, recipe);
	if (!file) {
		return {
			status: 127,
			stdout: "",
			stderr: "",
			missing: `recipe ${recipe} is in neither ${rel(ctx, join(procedure.dir, "justfile"))} nor the root justfile`,
		};
	}
	const just = findOnPath("just", ctx.io.env);
	if (!just) return { status: 127, stdout: "", stderr: "", missing: "`just` is not on PATH" };
	// A check (captured, not inherited) is bounded; a step's own recipe may take as long as it takes.
	return ctx.io.exec(
		just,
		["--justfile", file, "--working-directory", ctx.root, recipe, ...args],
		ctx.root,
		env,
		inherit,
		inherit ? undefined : CHECK_TIMEOUT_MS,
	);
}

function stepEnv(
	ctx: Ctx,
	procedure: Procedure,
	step: string,
	key: string,
	inputs: Map<string, string>,
) {
	const env: Record<string, string | undefined> = {
		...ctx.io.env,
		PROC_ID: procedure.id,
		PROC_STEP: step,
		PROC_KEY: key,
		PROC_DIR: procedure.dir,
		PROC_ROOT: ctx.root,
		PROC_RUNNER: RUNNER_FILE,
	};
	for (const [name, value] of inputs) env[inputVar(name)] = value;
	return env;
}

function tail(text: string, lines = 5): string {
	return text.trimEnd().split("\n").slice(-lines).join("\n");
}

// ---------------------------------------------------------------------------------------------------------------
// Commands

export const HELP = `proc: run, list, write and check pij procedures (PRD 005).

  proc [list] [--json]                          every procedure, from the files
  proc <proc> [--key <k>] [--json] [--clear]    a run's steps: done, next, to do (reads progress only)
  proc <proc> <step> [--arg name=value]… [--key <k>] [--again]
                                                run one step: inputs, order, preconditions, done check
  proc <proc> <step> --decided-by <who> --note <text>    record a decision step
  proc <proc> <step> --done [--note <text>]     mark a guide step done
  proc <proc> <command> [--arg name=value]…     run one of its commands (serve, watch); records nothing
  proc new <area> <slug> --title <t> [--kind runnable|mixed|guide|meta] [--owner <role>] [--skill]
  proc status <draft|current|superseded|retired> <proc>… --approved-by <who> [--why <text>] [--superseded-by <id>]
  proc check [--json]                           the drift check (harness checks stage "procedures")
  proc report [--json]                          files only one procedure uses; components no procedure operates

  --template <path>                             with list, a procedure or a step: a pod template's own procedures
                                                first (its folder's procedures/), then the repo's; list shows
                                                the ones it names, its own and linked, each found or MISSING

<proc> is an id (PROC-daemon-0001), a folder name or a slug; <step> is its id (s0001) or the digits (0001). Inputs also come from PROC_IN_<NAME> in the
environment; a secret input only from there. Exit: 0 ok, 1 failed, 2 usage, 3 refused (fix, then rerun).`;

/** Entry point. Returns the exit code. */
export function main(argv: string[], io: Io, options: Options = {}): number {
	const parsed = parseFlags(argv);
	if (!parsed.ok) {
		io.err(`proc: ${parsed.error}`);
		return EXIT_USAGE;
	}
	const flags = parsed.flags;
	const [first, ...rest] = flags.positionals;
	if (flags.help || first === "help") {
		io.out(HELP);
		return EXIT_OK;
	}
	if (flags.template !== undefined && ["new", "status", "check", "report"].includes(first ?? ""))
		return usageIo(io, `--template doesn't apply to ${first}`);
	const ctx = context(io, { ...options, template: flags.template ?? options.template });
	if (typeof ctx === "string") {
		io.err(`proc: ${ctx}`);
		return EXIT_USAGE;
	}
	if ((first === undefined || first === "list") && ctx.template)
		return templateListCmd(ctx, ctx.template, flags, rest);
	if (first === undefined || first === "list") return listCmd(ctx, flags, rest);
	if (first === "new") return newCmd(ctx, flags, rest);
	if (first === "status") return statusCmd(ctx, flags, rest);
	if (first === "check") return checkCmd(ctx, flags, rest);
	if (first === "report") return reportCmd(ctx, flags, rest);
	const found = findProcedure(ctx.loaded.procedures, first);
	if (found === undefined)
		return usage(ctx, `no procedure ${first} (list them: ${ctx.entry} list)`);
	if (Array.isArray(found))
		return usage(ctx, `${first} names ${found.map((p) => p.id).join(", ")}; use the id`);
	if (rest.length > 1) return usage(ctx, `one step at a time (got ${rest.join(" ")})`);
	const target = rest[0];
	if (target === undefined) return showCmd(ctx, flags, found);
	const step = found.steps.find((s) => s.id === target || s.id === `s${target}`);
	if (step) return stepCmd(ctx, flags, found, step);
	const command = found.commands.find((c) => c.name === target);
	if (command) return commandCmd(ctx, flags, found, command);
	return usage(
		ctx,
		`${found.id} has no step or command ${target} (steps: ${found.steps.map((s) => s.id).join(", ") || "none"}; commands: ${found.commands.map((c) => c.name).join(", ") || "none"})`,
	);
}

function usage(ctx: Ctx, message: string): number {
	return usageIo(ctx.io, message);
}

function usageIo(io: Io, message: string): number {
	io.err(`proc: ${message}`);
	return EXIT_USAGE;
}

function refuse(ctx: Ctx, message: string): number {
	ctx.io.err(`refused: ${message}`);
	return EXIT_REFUSED;
}

function fail(ctx: Ctx, message: string): number {
	ctx.io.err(`proc: ${message}`);
	return EXIT_FAIL;
}

// --- list (c-list, req-20)

function listCmd(ctx: Ctx, flags: Flags, rest: string[]): number {
	const bad = onlyFlags(flags, ["--json"], "list");
	if (bad) return usage(ctx, bad);
	if (rest.length > 0) return usage(ctx, `list takes no arguments (got ${rest.join(" ")})`);
	const rows = ctx.loaded.procedures
		.map((p) => ({
			id: p.id,
			title: p.title,
			kind: p.kind,
			status: p.status,
			owner: p.owner,
			home: p.home.kind,
			path: rel(ctx, p.dir),
		}))
		.sort((a, b) => a.id.localeCompare(b.id));
	if (flags.json) {
		ctx.io.out(JSON.stringify({ procedures: rows }, null, 2));
		return EXIT_OK;
	}
	if (rows.length === 0) {
		ctx.io.out(`no procedures yet (write one: ${ctx.entry} new <area> <slug> --title '<title>')`);
		return EXIT_OK;
	}
	const width = Math.max(...rows.map((r) => r.id.length));
	for (const row of rows) {
		ctx.io.out(
			`${row.id.padEnd(width)}  ${row.status.padEnd(10)} ${row.kind.padEnd(8)} ${row.title}  (${row.owner})`,
		);
	}
	return EXIT_OK;
}

// --- a pod template's procedures (PRD 010 req-01)

/** The template's own procedures and the ones it links, each found here or missing, with where it came from. */
export function templateRows(ctx: Ctx, template: TemplateInfo) {
	const rows = template.procedures.map((named) => {
		const found = ctx.loaded.procedures.find((p) => p.id === named.id);
		const home = found?.home.kind;
		const present =
			named.from === "template" ? home === "template" : home === "project" || home === "skill";
		return {
			id: named.id,
			from: named.from,
			use: named.use,
			present,
			title: present ? (found?.title ?? "") : "",
			path: present && found ? rel(ctx, found.dir) : "",
		};
	});
	for (const own of ctx.loaded.procedures.filter((p) => p.home.kind === "template")) {
		if (rows.some((row) => row.id === own.id)) continue;
		rows.push({
			id: own.id,
			from: "template",
			use: "",
			present: true,
			title: own.title,
			path: rel(ctx, own.dir),
		});
	}
	return rows;
}

function templateListCmd(ctx: Ctx, template: TemplateInfo, flags: Flags, rest: string[]): number {
	const bad = onlyFlags(flags, ["--json"], "list");
	if (bad) return usage(ctx, bad);
	if (rest.length > 0) return usage(ctx, `list takes no arguments (got ${rest.join(" ")})`);
	const rows = templateRows(ctx, template);
	if (flags.json) {
		ctx.io.out(
			JSON.stringify(
				{ template: rel(ctx, template.folder), name: template.name, procedures: rows },
				null,
				2,
			),
		);
		return EXIT_OK;
	}
	ctx.io.out(`pod template ${template.name || "(unnamed)"} at ${rel(ctx, template.folder)}`);
	if (rows.length === 0) {
		ctx.io.out("  it names no procedures");
		return EXIT_OK;
	}
	for (const row of rows) {
		const use = row.use ? ` [${row.use}]` : "";
		const where = row.from === "template" ? "its own" : "linked from the repo";
		ctx.io.out(
			row.present
				? `  ${row.id}${use}  ${row.title}  (${where}: ${row.path})`
				: `  ${row.id}${use}  MISSING (${where}): ${row.from === "template" ? `add procedures/<area>/${row.id}-<slug>/ to the template` : "this repo does not have it; copy it in or write one"}`,
		);
	}
	return EXIT_OK;
}

// --- a run's progress (req-14)

export interface StepState {
	id: string;
	title: string;
	kind: StepKind;
	state: "done" | "next" | "todo";
	record?: StepRecord;
}

export function stepStates(progress: Progress, procedure: Procedure, key: string): StepState[] {
	let next: string | undefined;
	return procedure.steps.map((step) => {
		const record = recordOf(progress, procedure.id, key, step.id);
		let state: StepState["state"] = record ? "done" : "todo";
		if (!record && next === undefined) {
			next = step.id;
			state = "next";
		}
		return { id: step.id, title: step.title, kind: step.kind, state, record };
	});
}

function keyFlag(key: string): string {
	return key === DEFAULT_KEY ? "" : ` --key ${key}`;
}

function showCmd(ctx: Ctx, flags: Flags, procedure: Procedure): number {
	const bad = onlyFlags(flags, ["--json", "--key", "--clear"], "a procedure's listing");
	if (bad) return usage(ctx, bad);
	const key = flags.key ?? DEFAULT_KEY;
	const io = ctx.io;
	if (flags.clear) {
		const cleared = clearRun(ctx.root, procedure.id, key);
		io.out(
			cleared
				? `cleared ${procedure.id} key ${key}`
				: `${procedure.id} has no progress under key ${key}`,
		);
		return EXIT_OK;
	}
	const steps = stepStates(readProgress(ctx.root), procedure, key);
	const next = steps.find((s) => s.state === "next");
	if (flags.json) {
		io.out(
			JSON.stringify(
				{
					id: procedure.id,
					title: procedure.title,
					status: procedure.status,
					kind: procedure.kind,
					key,
					steps,
					commands: procedure.commands.map((c) => ({ name: c.name, title: c.title })),
					next: next?.id ?? null,
					done: next === undefined,
				},
				null,
				2,
			),
		);
		return EXIT_OK;
	}
	io.out(
		`${procedure.id}  ${procedure.title}  (${procedure.status}, ${procedure.kind}, owner ${procedure.owner})`,
	);
	if (procedure.status === "superseded" || procedure.status === "retired") {
		io.out(
			`  note: this procedure is ${procedure.status}${procedure.supersededBy ? `; use ${procedure.supersededBy}` : ""}`,
		);
	}
	if (procedure.home.kind === "template")
		io.out(`from: the pod template's own, ${rel(ctx, procedure.dir)}`);
	if (procedure.who) io.out(`who: ${procedure.who}`);
	if (procedure.when) io.out(`when: ${procedure.when}`);
	io.out(`key: ${key}`);
	for (const step of steps) {
		const mark = step.state === "done" ? "✓" : step.state === "next" ? "→" : "·";
		const extra = step.record
			? `  (${step.record.how} ${step.record.at} by ${step.record.by})`
			: step.state === "next"
				? `  ${ctx.entry} ${procedure.id} ${step.id}${keyFlag(key)}`
				: "";
		io.out(
			`  ${mark} ${step.id} ${step.title}${step.kind === "run" ? "" : ` [${step.kind}]`}${extra}`,
		);
	}
	if (procedure.commands.length > 0) {
		io.out("commands (not steps; they record nothing):");
		for (const command of procedure.commands) io.out(`  ${command.name}  ${command.title}`);
	}
	io.out(
		next
			? `next: ${ctx.entry} ${procedure.id} ${next.id}${keyFlag(key)}`
			: `all steps done. Done when: ${procedure.doneWhen}`,
	);
	return EXIT_OK;
}

// --- running a step (c-runner)

function gatherInputs(ctx: Ctx, procedure: Procedure, flags: Flags): Map<string, string> | string {
	const values = new Map<string, string>();
	for (const input of procedure.inputs) {
		const fromEnv = ctx.io.env[inputVar(input.name)];
		if (fromEnv !== undefined && fromEnv !== "") values.set(input.name, fromEnv);
	}
	for (const [name, value] of flags.args) {
		const input = procedure.inputs.find((i) => i.name === name);
		if (!input)
			return `${procedure.id} has no input ${name} (inputs: ${procedure.inputs.map((i) => i.name).join(", ") || "none"})`;
		if (input.secret)
			return `input ${name} is secret: set ${inputVar(name)} in the environment, never on the command line`;
		values.set(name, value);
	}
	return values;
}

function printNext(ctx: Ctx, procedure: Procedure, key: string): void {
	const next = stepStates(readProgress(ctx.root), procedure, key).find((s) => s.state === "next");
	ctx.io.out(
		next
			? `next: ${ctx.entry} ${procedure.id} ${next.id}${keyFlag(key)}`
			: `all steps done. Done when: ${procedure.doneWhen}`,
	);
}

/** Resolve a `requires` link (`<path to procedure.dd.json>#steps/<id>`, relative to the procedure's folder). */
function resolveRequire(
	ctx: Ctx,
	procedure: Procedure,
	link: string,
): { procedure: Procedure; step: Step } | undefined {
	const [path, anchor] = link.split("#");
	const stepId = /^steps\/(s\d{4})$/.exec(anchor ?? "")?.[1];
	if (!path || !stepId) return undefined;
	const file = resolve(procedure.dir, path);
	const target = ctx.loaded.procedures.find((p) => p.file === file);
	const step = target?.steps.find((s) => s.id === stepId);
	return target && step ? { procedure: target, step } : undefined;
}

function stepCmd(ctx: Ctx, flags: Flags, procedure: Procedure, step: Step): number {
	const bad = onlyFlags(
		flags,
		["--key", "--arg", "--again", "--done", "--decided-by", "--note"],
		"a step",
	);
	if (bad) return usage(ctx, bad);
	if (flags.again && step.kind !== "run")
		return usage(ctx, `--again only reruns a run step; ${step.id} is a ${step.kind} step`);
	if (flags.done && step.kind !== "guide")
		return usage(ctx, `--done only marks a guide step; ${step.id} is a ${step.kind} step`);
	if (flags.decidedBy !== undefined && step.kind !== "decision") {
		return usage(
			ctx,
			`--decided-by only records a decision step; ${step.id} is a ${step.kind} step`,
		);
	}
	if (flags.decidedBy !== undefined && !flags.note)
		return usage(ctx, "--decided-by needs --note '<the decision>'");
	const io = ctx.io;
	const key = flags.key ?? DEFAULT_KEY;
	const inputs = gatherInputs(ctx, procedure, flags);
	if (typeof inputs === "string") return usage(ctx, inputs);
	for (const name of step.inputs) {
		const input = procedure.inputs.find((i) => i.name === name);
		if (!input)
			return fail(
				ctx,
				`step ${step.id} names input ${name}, which ${procedure.id} doesn't declare`,
			);
		if (input.required && !inputs.has(name)) {
			const how = input.secret
				? `set ${inputVar(name)} in the environment`
				: `pass --arg ${name}=<value>`;
			const fallback = input.fallback ? ` If you don't have it: ${input.fallback}` : "";
			return refuse(
				ctx,
				`step ${step.id} needs input ${name} (${input.description}): ${how}.${fallback}`,
			);
		}
	}
	const envFor = (p: Procedure, s: Step) => stepEnv(ctx, p, s.id, key, inputs);
	// Another procedure's done check gets this run's inputs, but not a secret one it doesn't declare itself (req-13).
	const envForOther = (p: Procedure, s: Step) => {
		const secret = procedure.inputs.filter(
			(i) => i.secret && !p.inputs.some((theirs) => theirs.name === i.name),
		);
		const env = stepEnv(
			ctx,
			p,
			s.id,
			key,
			new Map([...inputs].filter(([name]) => !secret.some((i) => i.name === name))),
		);
		for (const input of secret) delete env[inputVar(input.name)];
		return env;
	};
	const now = () => io.now().toISOString();
	if (step.who) io.out(`who: ${step.who}`);

	// Order inside the procedure: each earlier step is recorded, or its done check passes now.
	const progress = readProgress(ctx.root);
	for (const earlier of procedure.steps) {
		if (earlier.id === step.id) break;
		if (recordOf(progress, procedure.id, key, earlier.id)) continue;
		if (earlier.kind === "run" && earlier.done) {
			const check = runRecipe(ctx, procedure, earlier.done, envFor(procedure, earlier), false);
			if (check.missing) return fail(ctx, check.missing);
			if (check.status === 0) {
				saveRecord(ctx.root, procedure.id, key, earlier.id, {
					at: now(),
					by: by(io),
					how: "found-done",
				});
				continue;
			}
		}
		return refuse(
			ctx,
			`step ${earlier.id} (${earlier.title}) isn't done yet: run ${ctx.entry} ${procedure.id} ${earlier.id}${keyFlag(key)} first`,
		);
	}

	// Order across procedures: the other step's live done check first, its recorded progress only without one (req-12).
	for (const link of step.requires) {
		const required = resolveRequire(ctx, procedure, link);
		if (!required)
			return fail(
				ctx,
				`step ${step.id} requires ${link}, which doesn't resolve to a procedure step`,
			);
		const { procedure: other, step: otherStep } = required;
		if (otherStep.kind === "run" && otherStep.done) {
			const check = runRecipe(ctx, other, otherStep.done, envForOther(other, otherStep), false);
			if (check.missing) return fail(ctx, check.missing);
			if (check.status === 0) continue;
		} else if (
			recordOf(progress, other.id, key, otherStep.id) ||
			recordOf(progress, other.id, DEFAULT_KEY, otherStep.id)
		) {
			continue;
		}
		return refuse(
			ctx,
			`step ${step.id} needs ${other.id} step ${otherStep.id} (${otherStep.title}) first: ${ctx.entry} ${other.id} ${otherStep.id}`,
		);
	}

	// Preconditions: the procedure's, then the step's. Each is a recipe that refuses with its fix (req-11).
	const checks = [
		...procedure.preconditions.map((p) => ({
			label: `precondition ${p.id} (${p.text})`,
			check: p.check,
			fix: p.fix,
		})),
		...step.pre.map((p) => ({
			label: `step ${step.id} precondition (${p.check})`,
			check: p.check,
			fix: p.fix,
		})),
	];
	for (const pre of checks) {
		const check = runRecipe(ctx, procedure, pre.check, envFor(procedure, step), false);
		if (check.missing) return fail(ctx, check.missing);
		if (check.status !== 0) {
			const output = tail(`${check.stdout}${check.stderr}`);
			return refuse(ctx, `${pre.label} fails${output ? `:\n${output}` : ""}\n  fix: ${pre.fix}`);
		}
	}

	const record = recordOf(progress, procedure.id, key, step.id);
	if (step.kind === "decision") {
		if (flags.decidedBy !== undefined) {
			saveRecord(ctx.root, procedure.id, key, step.id, {
				at: now(),
				by: flags.decidedBy,
				how: "decided",
				note: flags.note,
			});
			io.out(`recorded: ${flags.decidedBy} decided: ${flags.note ?? ""}`);
			printNext(ctx, procedure, key);
			return EXIT_OK;
		}
		if (record) {
			io.out(`already decided by ${record.by} at ${record.at}: ${record.note ?? ""}`);
			printNext(ctx, procedure, key);
			return EXIT_OK;
		}
		return refuse(
			ctx,
			`step ${step.id} needs a decision from ${step.decide ?? "a person"}: ${step.doneWhen}\n  record it: ${ctx.entry} ${procedure.id} ${step.id}${keyFlag(key)} --decided-by '<who>' --note '<the decision>'`,
		);
	}
	if (step.kind === "guide") {
		if (flags.done) {
			saveRecord(ctx.root, procedure.id, key, step.id, {
				at: now(),
				by: by(io),
				how: "marked",
				note: flags.note,
			});
			io.out(`marked ${step.id} done`);
			printNext(ctx, procedure, key);
			return EXIT_OK;
		}
		if (record) {
			io.out(`already done (${record.how} ${record.at} by ${record.by})`);
			printNext(ctx, procedure, key);
			return EXIT_OK;
		}
		const follow = step.follow ?? "";
		const sub = findProcedure(ctx.loaded.procedures, follow);
		if (sub && !Array.isArray(sub)) {
			const states = stepStates(readProgress(ctx.root), sub, key);
			if (states.length > 0 && states.every((s) => s.state === "done")) {
				saveRecord(ctx.root, procedure.id, key, step.id, {
					at: now(),
					by: by(io),
					how: "found-done",
					note: `${sub.id} done`,
				});
				io.out(`${sub.id} is done under key ${key}, so ${step.id} is done`);
				printNext(ctx, procedure, key);
				return EXIT_OK;
			}
			return refuse(
				ctx,
				`step ${step.id} follows ${sub.id}: ${ctx.entry} ${sub.id}${keyFlag(key)}. It is done when every step there is, or mark it: --done`,
			);
		}
		return refuse(
			ctx,
			`step ${step.id}: read and follow ${rel(ctx, followPath(ctx.root, procedure, follow))}, then mark it: ${ctx.entry} ${procedure.id} ${step.id}${keyFlag(key)} --done`,
		);
	}

	// A run step: skip it when its done check already passes, run it, then require the done check (req-09, req-13).
	if (!flags.again) {
		if (step.done) {
			const check = runRecipe(ctx, procedure, step.done, envFor(procedure, step), false);
			if (check.missing) return fail(ctx, check.missing);
			if (check.status === 0) {
				if (!record)
					saveRecord(ctx.root, procedure.id, key, step.id, {
						at: now(),
						by: by(io),
						how: "found-done",
					});
				io.out(`already done (${step.doneWhen}) (--again to run it anyway)`);
				printNext(ctx, procedure, key);
				return EXIT_OK;
			}
		} else if (record) {
			io.out(
				`already done (${record.how} ${record.at} by ${record.by}) (--again to run it anyway)`,
			);
			printNext(ctx, procedure, key);
			return EXIT_OK;
		}
	}
	if (!step.run) return fail(ctx, `run step ${step.id} has no run recipe`);
	io.out(`running ${procedure.id} ${step.id}: ${step.title}`);
	const ran = runRecipe(ctx, procedure, step.run, envFor(procedure, step), true);
	if (ran.missing) return fail(ctx, ran.missing);
	if (ran.status !== 0)
		return fail(ctx, `step ${step.id} failed (exit ${ran.status}); nothing recorded`);
	if (step.done) {
		const check = runRecipe(ctx, procedure, step.done, envFor(procedure, step), false);
		if (check.status !== 0) {
			const output = tail(`${check.stdout}${check.stderr}`);
			return fail(
				ctx,
				`step ${step.id} ran, but its done check (${step.done}) fails: ${step.doneWhen}${output ? `\n${output}` : ""}\nnothing recorded`,
			);
		}
	}
	saveRecord(ctx.root, procedure.id, key, step.id, { at: now(), by: by(io), how: "ran" });
	io.out(`✓ ${step.id} done`);
	printNext(ctx, procedure, key);
	return EXIT_OK;
}

// --- commands outside the steps (req-16)

function commandCmd(ctx: Ctx, flags: Flags, procedure: Procedure, command: Command): number {
	const bad = onlyFlags(flags, ["--arg", "--key"], "a command");
	if (bad) return usage(ctx, bad);
	const inputs = gatherInputs(ctx, procedure, flags);
	if (typeof inputs === "string") return usage(ctx, inputs);
	const env = stepEnv(ctx, procedure, command.name, flags.key ?? DEFAULT_KEY, inputs);
	const run = runRecipe(ctx, procedure, command.run, env, true);
	if (run.missing) return fail(ctx, run.missing);
	return run.status;
}

// --- new (c-booking, req-02, req-07)

function procedureSkeleton(
	id: string,
	area: string,
	title: string,
	kind: string,
	owner: string,
	date: string,
): Json {
	const step =
		kind === "guide"
			? {
					id: "s0001",
					title: "<first step>",
					kind: "guide",
					follow: "<a procedure id or a repo path>",
					done_when: "<what is true after it>",
				}
			: {
					id: "s0001",
					title: "<first step>",
					kind: "run",
					run: "step-0001",
					done: "step-0001-done",
					done_when: "<what is true after it>",
				};
	return {
		dd: { schema: SCHEMA },
		sections: [
			{
				name: "meta",
				value: {
					id,
					title,
					area,
					kind,
					status: "draft",
					owner,
					updated: date,
					summary: "<one or two sentences: what this does and why>",
				},
			},
			{ name: "who_when", value: { who: "<the role that runs it>", when: "<what starts it>" } },
			{ name: "inputs", value: [] },
			{ name: "preconditions", value: [] },
			{ name: "steps", value: [step] },
			{ name: "done_when", value: "<what is true when the whole procedure is done>" },
			{ name: "known_limits", value: [] },
			{ name: "mistakes", value: [] },
			{ name: "uses", value: [] },
			{ name: "side_effects", value: [] },
			{ name: "comments", value: [] },
			{ name: "changelog", value: [{ date, by: owner, change: "First draft." }] },
		],
		references: [],
	};
}

function justfileSkeleton(id: string): string {
	return `# ${id}'s own recipes. The procedure runner runs this file directly (\`just --justfile <this file>
# --working-directory <repo root>\`); the root justfile never imports it. Each recipe gets PROC_ROOT, PROC_DIR,
# PROC_KEY, PROC_STEP, PROC_IN_<NAME> for each input, and PROC_RUNNER (the runner itself:
# node --experimental-strip-types "$PROC_RUNNER" check).

# Step s0001: replace with the real work.
step-0001:
    @echo "${id} step 0001 is not written yet" >&2; exit 1

# Step s0001's done check: exit 0 only when its result is really there.
step-0001-done:
    @exit 1
`;
}

/** The highest number any folder in `homes` holds for `area`. */
export function highestNumber(homes: Home[], area: string): number {
	let highest = 0;
	for (const home of homes) {
		const dir = join(home.dir, area);
		if (!isDir(dir)) continue;
		for (const name of readdirSync(dir)) {
			const match = FOLDER_RE.exec(name);
			if (match?.[1] === area) highest = Math.max(highest, Number(match[2]));
		}
	}
	return highest;
}

function newCmd(ctx: Ctx, flags: Flags, rest: string[]): number {
	const bad = onlyFlags(flags, ["--title", "--kind", "--owner", "--skill", "--json"], "new");
	if (bad) return usage(ctx, bad);
	const [area, slug, ...extra] = rest;
	if (!area || !slug || extra.length > 0) {
		return usage(
			ctx,
			"new <area> <slug> --title '<title>' [--kind runnable|mixed|guide|meta] [--owner <role>] [--skill]",
		);
	}
	if (!AREA_RE.test(area)) return usage(ctx, `area ${area}: lowercase letters and digits`);
	if (!SLUG_RE.test(slug)) return usage(ctx, `slug ${slug}: lowercase words joined by -`);
	if (!flags.title) return usage(ctx, "new needs --title '<title>'");
	const kind = flags.kind ?? "runnable";
	if (!KINDS.includes(kind)) return usage(ctx, `--kind is one of ${KINDS.join(", ")}`);
	const io = ctx.io;
	const git = gitState(ctx.root);
	if (git.branch === "main" || git.branch === "master") {
		return refuse(
			ctx,
			`${ctx.root} is on ${git.branch}: write procedures on a feature branch in its own worktree`,
		);
	}
	if (git.mainCheckout)
		return refuse(
			ctx,
			`${ctx.root} is the main checkout: write procedures in a feature worktree (git worktree add)`,
		);
	if (!git.branch) return refuse(ctx, `${ctx.root} is not on a branch`);
	const repoSkill = join(ctx.root, SKILL_HOME);
	let home: Home;
	if (flags.skill) {
		if (!isPijRepo(ctx.root))
			return refuse(
				ctx,
				"--skill writes into pij's own repo (skills/pij/procedures); this checkout isn't pij's",
			);
		if (!SKILL_AREAS.includes(area))
			return refuse(
				ctx,
				`the skill's areas are ${SKILL_AREAS.join(", ")}; add ${area} to SKILL_AREAS in the runner first`,
			);
		home = { kind: "skill", dir: repoSkill };
	} else {
		if (SKILL_AREAS.includes(area) && isPijRepo(ctx.root)) {
			return refuse(ctx, `area ${area} belongs to the pij skill's procedures here: add --skill`);
		}
		home = { kind: "project", dir: join(ctx.root, ctx.config.home) };
	}
	const pij = findOnPath("pij", io.env);
	if (!pij) return refuse(ctx, "`pij` is not on PATH: numbers are booked with `pij claim`");
	const claim = io.exec(
		pij,
		["claim", `procedure-${area}`, "--for", slug],
		ctx.root,
		io.env,
		false,
	);
	const number = Number(/(\d+) claimed/.exec(`${claim.stdout}${claim.stderr}`)?.[1]);
	if (claim.status !== 0 || !Number.isInteger(number) || number <= 0) {
		return fail(
			ctx,
			`pij claim procedure-${area} failed:\n${tail(`${claim.stdout}${claim.stderr}`)}`,
		);
	}
	const highest = highestNumber(ctx.homes, area);
	if (number <= highest) {
		return refuse(
			ctx,
			`pij claim handed out ${number}, but a folder already holds ${area} ${highest}. Give it back (pij claim release procedure-${area} ${number} --why 'a folder holds ${highest}') and claim again until the number passes ${highest}.`,
		);
	}
	const id = `PROC-${area}-${String(number).padStart(4, "0")}`;
	const dir = join(home.dir, area, `${id}-${slug}`);
	if (existsSync(dir)) return refuse(ctx, `${rel(ctx, dir)} already exists`);
	const date = localDate(io.now());
	const owner = flags.owner ?? io.env.PIJ_SESSION_ID ?? "<owner role>";
	mkdirSync(dir, { recursive: true });
	const doc = join(dir, DOC_FILE);
	writeFileSync(
		doc,
		`${JSON.stringify(procedureSkeleton(id, area, flags.title, kind, owner, date), null, "\t")}\n`,
	);
	const files = [doc];
	if (kind !== "guide") {
		writeFileSync(join(dir, "justfile"), justfileSkeleton(id));
		files.push(join(dir, "justfile"));
	}
	const gitBin = findOnPath("git", io.env);
	const ddocs = findOnPath("ddocs", io.env);
	if (gitBin)
		io.exec(gitBin, ["add", "--", ...files.map((f) => rel(ctx, f))], ctx.root, io.env, false);
	if (ddocs) {
		const validate = io.exec(ddocs, ["validate", rel(ctx, doc)], ctx.root, io.env, false);
		if (validate.status !== 0)
			io.err(`ddocs validate ${rel(ctx, doc)} failed:\n${tail(validate.stdout + validate.stderr)}`);
		io.exec(ddocs, ["build", rel(ctx, doc)], ctx.root, io.env, false);
		if (gitBin)
			io.exec(
				gitBin,
				["add", "--", rel(ctx, join(dir, "procedure.dd.md"))],
				ctx.root,
				io.env,
				false,
			);
	} else {
		io.err("ddocs is not on PATH: validate and build the new document yourself");
	}
	if (flags.json)
		io.out(JSON.stringify({ id, path: rel(ctx, dir), worktree: ctx.root, branch: git.branch }));
	else {
		io.out(`created ${id} at ${rel(ctx, dir)}`);
		io.out(
			`worktree: ${ctx.root} (branch ${git.branch}); staged: ${files.map((f) => basenameOf(f)).join(", ")}`,
		);
		io.out(`next: replace every <placeholder>, then ${ctx.entry} check`);
	}
	return EXIT_OK;
}

// --- status (c-status, req-21)

function statusCmd(ctx: Ctx, flags: Flags, rest: string[]): number {
	const bad = onlyFlags(flags, ["--approved-by", "--why", "--superseded-by"], "status");
	if (bad) return usage(ctx, bad);
	const [status, ...refs] = rest;
	if (!status || !STATUSES.includes(status) || refs.length === 0)
		return usage(ctx, `status <${STATUSES.join("|")}> <proc>… --approved-by <who>`);
	if (!flags.approvedBy) return usage(ctx, "status needs --approved-by '<who said yes>'");
	if (!flags.why)
		return usage(
			ctx,
			"status needs --why '<their words, quoted>' so a scoped yes isn't recorded as a full one",
		);
	if (status === "superseded" && !flags.supersededBy)
		return usage(ctx, "superseded needs --superseded-by <id>");
	const procedures: Procedure[] = [];
	for (const ref of refs) {
		const found = findProcedure(ctx.loaded.procedures, ref);
		if (!found || Array.isArray(found)) return usage(ctx, `no single procedure ${ref}`);
		procedures.push(found);
	}
	const ddocs = findOnPath("ddocs", ctx.io.env);
	if (!ddocs) return refuse(ctx, "ddocs is not on PATH: status changes are written through ddocs");
	const date = localDate(ctx.io.now());
	const recorder = by(ctx.io);
	for (const procedure of procedures) {
		const doc = rel(ctx, procedure.file);
		const why = flags.why ? ` (${flags.why})` : "";
		const successor = flags.supersededBy ? `; superseded by ${flags.supersededBy}` : "";
		const change = `Status ${procedure.status} → ${status}; approved by ${flags.approvedBy}${why}${successor}; recorded by ${recorder}.`;
		const edits: string[][] = [
			["set", `${doc}#meta/status`, status],
			["set", `${doc}#meta/updated`, date],
			...(flags.supersededBy ? [["set", `${doc}#meta/superseded_by`, flags.supersededBy]] : []),
			["add", `${doc}#changelog`, JSON.stringify({ date, by: flags.approvedBy, change })],
			["build", doc],
		];
		for (const edit of edits) {
			const result = ctx.io.exec(ddocs, edit, ctx.root, ctx.io.env, false);
			if (result.status !== 0)
				return fail(
					ctx,
					`ddocs ${edit.slice(0, 2).join(" ")} failed:\n${tail(result.stdout + result.stderr)}`,
				);
		}
		ctx.io.out(
			`${procedure.id}: ${procedure.status} → ${status} (approved by ${flags.approvedBy})`,
		);
	}
	return EXIT_OK;
}

// --- check (c-check, req-19 and the rows it guards)

/** Numbered items in markdown that each begin with a command: inline code, or a code block under the item (q-09). */
export function commandItems(text: string): number {
	const lines = text.split("\n");
	let count = 0;
	let fence = false;
	for (let index = 0; index < lines.length; index++) {
		const line = lines[index] ?? "";
		if (line.trimStart().startsWith("```")) {
			fence = !fence;
			continue;
		}
		if (fence) continue;
		const item = /^ {0,3}\d+[.)]\s+(.*)$/.exec(line);
		if (!item) continue;
		if ((item[1] ?? "").trim().startsWith("`")) {
			count++;
			continue;
		}
		let next = index + 1;
		while (next < lines.length && (lines[next] ?? "").trim() === "") next++;
		if (/^\s+```/.test(lines[next] ?? "")) count++;
	}
	return count;
}

/** Repo paths a text names that exist in `tracked`: each path-shaped token, or its tail past leading segments. */
export function pathsIn(text: string, tracked: Set<string>): Set<string> {
	const found = new Set<string>();
	for (const match of text.matchAll(/[A-Za-z0-9_.@-]+(?:\/[A-Za-z0-9_.@-]+)+/g)) {
		const parts = match[0].replace(/^\.\//, "").split("/");
		for (let start = 0; start < parts.length - 1; start++) {
			const candidate = parts.slice(start).join("/");
			if (tracked.has(candidate)) {
				found.add(candidate);
				break;
			}
		}
	}
	return found;
}

function walkFiles(dir: string): string[] {
	const files: string[] = [];
	if (!isDir(dir)) return files;
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (isDir(path)) files.push(...walkFiles(path));
		else files.push(path);
	}
	return files;
}

function commandsOf(procedure: Procedure): { where: string; command: string }[] {
	const commands: { where: string; command: string }[] = [];
	for (const p of procedure.preconditions)
		commands.push({ where: `precondition ${p.id}`, command: p.check });
	for (const step of procedure.steps) {
		if (step.run) commands.push({ where: `step ${step.id} run`, command: step.run });
		if (step.done) commands.push({ where: `step ${step.id} done`, command: step.done });
		for (const p of step.pre) commands.push({ where: `step ${step.id} pre`, command: p.check });
	}
	for (const c of procedure.commands) commands.push({ where: `command ${c.name}`, command: c.run });
	return commands;
}

/** Repo paths outside its folder that a procedure's steps and scripts use. */
function usedPaths(ctx: Ctx, procedure: Procedure, tracked: Set<string>): Set<string> {
	const folder = rel(ctx, procedure.dir);
	const texts = commandsOf(procedure).map((c) => c.command);
	for (const file of walkFiles(procedure.dir)) {
		if (SCRIPT_RE.test(basenameOf(file))) texts.push(readText(file) ?? "");
	}
	const used = new Set<string>();
	for (const text of texts) {
		for (const path of pathsIn(text, tracked)) {
			if (!path.startsWith(`${folder}/`)) used.add(path);
		}
	}
	return used;
}

function trackedFiles(ctx: Ctx): Set<string> | string {
	const git = findOnPath("git", ctx.io.env);
	if (!git) return "git is not on PATH";
	const result = ctx.io.exec(git, ["ls-files", "-z"], ctx.root, ctx.io.env, false);
	if (result.status !== 0) return `git ls-files failed: ${tail(result.stderr)}`;
	return new Set(result.stdout.split("\0").filter((f) => f !== ""));
}

/** Every drift finding for this checkout (empty when clean). */
export function checkFindings(ctx: Ctx): string[] {
	const findings = [...ctx.loaded.problems];
	const tracked = trackedFiles(ctx);
	if (typeof tracked === "string") return [...findings, tracked];
	const homeDirs = ctx.homes.map((h) => rel(ctx, h.dir));
	const inHome = (path: string) => homeDirs.some((h) => path.startsWith(`${h}/`));

	// Recipe names, as just itself reads them.
	const just = findOnPath("just", ctx.io.env);
	if (!just) findings.push("`just` is not on PATH: recipe names were not checked");
	const summaries = new Map<string, Set<string> | undefined>();
	const summary = (file: string): Set<string> | undefined => {
		if (!summaries.has(file) && just) {
			const result = ctx.io.exec(
				just,
				["--justfile", file, "--summary"],
				ctx.root,
				ctx.io.env,
				false,
			);
			if (result.status !== 0)
				findings.push(`${rel(ctx, file)}: just can't read it: ${tail(result.stderr, 2)}`);
			summaries.set(
				file,
				result.status === 0
					? new Set(result.stdout.split(/\s+/).filter((n) => n !== ""))
					: undefined,
			);
		}
		return summaries.get(file);
	};
	const rootFile = rootJustfile(ctx.root);
	const rootRecipes = rootFile ? summary(rootFile) : undefined;

	for (const path of tracked) {
		if (basenameOf(path) === DOC_FILE && !inHome(path))
			findings.push(`${path}: a procedure document outside a procedure home`);
	}

	// Only this repo's procedures: the skill's own, seen from another repo, are pij's to check.
	for (const procedure of ctx.loaded.procedures.filter((p) => p.dir.startsWith(ctx.root + sep))) {
		const where = rel(ctx, procedure.file);
		if (!KINDS.includes(procedure.kind))
			findings.push(`${where}: meta.kind ${procedure.kind} is not one of ${KINDS.join(", ")}`);
		const own = join(procedure.dir, "justfile");
		const ownRecipes = isFile(own) ? summary(own) : undefined;
		const ids = new Set<string>();
		for (const step of procedure.steps) {
			if (!STEP_ID_RE.test(step.id))
				findings.push(`${where}: step id ${step.id} is not s and four digits (s0001)`);
			if (ids.has(step.id)) findings.push(`${where}: step id ${step.id} is used twice`);
			ids.add(step.id);
			if (step.kind === "run" && !step.run)
				findings.push(`${where}: run step ${step.id} has no run recipe`);
			if (step.kind === "decision" && !step.decide)
				findings.push(`${where}: decision step ${step.id} doesn't say who decides`);
			if (step.kind === "guide") {
				const follow = step.follow ?? "";
				if (
					!findProcedure(ctx.loaded.procedures, follow) &&
					!(follow && existsSync(followPath(ctx.root, procedure, follow)))
				) {
					findings.push(
						`${where}: guide step ${step.id} follows ${follow || "nothing"}, which is no procedure and no path`,
					);
				}
			}
			if (step.changesAccess && !step.done)
				findings.push(
					`${where}: step ${step.id} changes access but has no done check that reads the live state`,
				);
			for (const name of step.inputs) {
				if (!procedure.inputs.some((i) => i.name === name))
					findings.push(`${where}: step ${step.id} names input ${name}, which isn't declared`);
			}
			for (const link of step.requires) {
				if (!resolveRequire(ctx, procedure, link))
					findings.push(`${where}: step ${step.id} requires ${link}, which is no procedure step`);
			}
		}
		if (just) {
			for (const { where: at, command } of commandsOf(procedure)) {
				const recipe = splitWords(command)[0] ?? "";
				if (!(ownRecipes?.has(recipe) || rootRecipes?.has(recipe))) {
					findings.push(
						`${where}: ${at} names recipe ${recipe || "(none)"}, which is in neither its justfile nor the root justfile`,
					);
				}
			}
		}
		const declared = [...procedure.uses, ...procedure.sideEffects].map((u) =>
			u.path.replace(/\/$/, ""),
		);
		for (const path of usedPaths(ctx, procedure, tracked)) {
			if (!declared.some((d) => path === d || path.startsWith(`${d}/`))) {
				findings.push(
					`${where}: uses ${path} outside its folder without listing it in uses or side_effects`,
				);
			}
		}
		for (const file of walkFiles(procedure.dir)) {
			if ((readText(file) ?? "").includes(RELAYED_MESSAGE)) {
				findings.push(
					`${rel(ctx, file)}: holds a relayed pij message; never store message bodies in the repo`,
				);
			}
		}
	}

	// Numbered commands outside a procedure folder (req-31).
	for (const path of [...tracked].sort()) {
		const legacy = ctx.config.legacyPages.get(path);
		if (!path.endsWith(".md") || path.endsWith(".dd.md")) continue;
		if (inHome(path) && legacy === undefined) continue;
		if (![...NUMBERED_SCAN_ROOTS, ctx.config.home].some((r) => path.startsWith(`${r}/`))) continue;
		const text = readText(join(ctx.root, path)) ?? "";
		if (NOT_A_PROCEDURE_RE.test(text)) continue;
		const count = commandItems(text);
		if (legacy !== undefined) {
			if (count > legacy) {
				findings.push(
					`${path}: a legacy page in ${CONFIG_FILE} grew from ${legacy} to ${count} numbered commands; it may only shrink (move the new steps into a procedure)`,
				);
			}
			continue;
		}
		if (count >= NUMBERED_COMMAND_LIMIT) {
			findings.push(
				`${path}: ${count} numbered steps begin with a command; make it a procedure, or say why it is reference with <!-- not-a-procedure: <reason> -->`,
			);
		}
	}
	return findings;
}

function checkCmd(ctx: Ctx, flags: Flags, rest: string[]): number {
	const bad = onlyFlags(flags, ["--json"], "check");
	if (bad) return usage(ctx, bad);
	if (rest.length > 0) return usage(ctx, "check takes no arguments");
	const findings = checkFindings(ctx);
	if (flags.json)
		ctx.io.out(
			JSON.stringify(
				{ ok: findings.length === 0, procedures: ctx.loaded.procedures.length, findings },
				null,
				2,
			),
		);
	else if (findings.length === 0)
		ctx.io.out(`procedures-check: ${ctx.loaded.procedures.length} procedure(s) clean`);
	else {
		ctx.io.out(`procedures-check: ${findings.length} finding(s)`);
		for (const finding of findings) ctx.io.out(`  ${finding}`);
	}
	return findings.length === 0 ? EXIT_OK : EXIT_FAIL;
}

// --- report (c-report, req-06)

function reportCmd(ctx: Ctx, flags: Flags, rest: string[]): number {
	const bad = onlyFlags(flags, ["--json"], "report");
	if (bad) return usage(ctx, bad);
	if (rest.length > 0) return usage(ctx, "report takes no arguments");
	const tracked = trackedFiles(ctx);
	if (typeof tracked === "string") return refuse(ctx, tracked);
	const git = findOnPath("git", ctx.io.env);
	const users = new Map<string, Procedure[]>();
	for (const procedure of ctx.loaded.procedures) {
		for (const path of usedPaths(ctx, procedure, tracked))
			users.set(path, [...(users.get(path) ?? []), procedure]);
	}
	const homeDirs = ctx.homes.map((h) => rel(ctx, h.dir));
	const colocate: { path: string; belongs: string }[] = [];
	for (const [path, procedures] of [...users].sort(([a], [b]) => a.localeCompare(b))) {
		const only = procedures[0];
		if (procedures.length !== 1 || !only || !git) continue;
		const grep = ctx.io.exec(git, ["grep", "-l", "-F", "--", path], ctx.root, ctx.io.env, false);
		const others = grep.stdout
			.split("\n")
			.filter((f) => f !== "" && f !== path && !homeDirs.some((h) => f.startsWith(`${h}/`)));
		if (others.length === 0) colocate.push({ path, belongs: rel(ctx, only.dir) });
	}
	const operated = new Set<string>();
	for (const procedure of ctx.loaded.procedures) {
		for (const link of procedure.operates) {
			const [path, anchor] = link.split("#");
			if (path && anchor) operated.add(`${basenameOf(resolve(procedure.dir, path))}#${anchor}`);
		}
	}
	const unoperated: { blueprint: string; component: string; kind: string }[] = [];
	const blueprints = join(ctx.root, "docs/project/blueprints");
	for (const file of isDir(blueprints) ? readdirSync(blueprints).sort() : []) {
		if (!file.endsWith(".dd.json")) continue;
		let doc: unknown;
		try {
			doc = JSON.parse(readText(join(blueprints, file)) ?? "");
		} catch {
			continue;
		}
		for (const component of list(sectionMap(doc).get("components")).filter(isObj)) {
			const id = str(component.id) ?? "";
			const kind = str(component.kind) ?? "";
			if (OPERATED_KINDS.includes(kind) && !operated.has(`${file}#components/${id}`))
				unoperated.push({ blueprint: file, component: id, kind });
		}
	}
	if (flags.json) {
		ctx.io.out(JSON.stringify({ colocate, unoperated }, null, 2));
		return EXIT_OK;
	}
	ctx.io.out(`Files only one procedure uses, kept outside its folder (${colocate.length}):`);
	for (const c of colocate) ctx.io.out(`  ${c.path}  → belongs in ${c.belongs}`);
	ctx.io.out(
		`Blueprint components (${OPERATED_KINDS.join(", ")}) no procedure operates (${unoperated.length}):`,
	);
	for (const u of unoperated) ctx.io.out(`  ${u.blueprint}#components/${u.component} (${u.kind})`);
	return EXIT_OK;
}

// ---------------------------------------------------------------------------------------------------------------
// Real side effects

// ---------------------------------------------------------------------------------------------------------------
// A pod's close procedure (PRD 010 req-01)

export interface RunThrough {
	done: boolean;
	code: number;
	/** The step it stopped at, when it did. */
	stoppedAt?: string;
}

/** Run each step not yet done, in order, as `proc <proc> <step>` would, until all are done or one stops. */
export function runThrough(ctx: Ctx, procedure: Procedure, key: string): RunThrough {
	const parsed = parseFlags(["--key", key]);
	if (!parsed.ok) return { done: false, code: EXIT_USAGE };
	for (let guard = 0; guard <= procedure.steps.length; guard++) {
		const next = stepStates(readProgress(ctx.root), procedure, key).find((s) => s.state === "next");
		if (!next) return { done: true, code: EXIT_OK };
		const step = procedure.steps.find((s) => s.id === next.id);
		if (!step) return { done: false, code: EXIT_FAIL, stoppedAt: next.id };
		const code = stepCmd(ctx, parsed.flags, procedure, step);
		if (code !== EXIT_OK) return { done: false, code, stoppedAt: step.id };
		if (!recordOf(readProgress(ctx.root), procedure.id, key, step.id))
			return { done: false, code: EXIT_FAIL, stoppedAt: step.id };
	}
	return { done: false, code: EXIT_FAIL };
}

/**
 * `pij pod down <pod>` through the pij CLI: when the pod's template names a close procedure, run it to its end
 * here, in the caller's process, then ask pij-rs for down. If it stops early, down closes nothing: a brake (removing
 * it only closes more), with `--skip-close-procedure` to go on without it. `args` is the whole command line, `rest`
 * what follows `pod`. Launches one `pij-rs pod show`, the procedure's own steps, then the down itself.
 */
export function podDown(
	args: string[],
	rest: string[],
	io: Io,
	pijRs: string,
	options: Options = {},
): number {
	const forward = (extra: string[]) =>
		io.exec(pijRs, [...args, ...extra], io.cwd, io.env, true).status;
	const words = rest.slice(1);
	if (words.some((w) => w === "--help" || w === "-h" || w === "--skip-close-procedure"))
		return forward([]);
	const pod = words.find((w) => !w.startsWith("-"));
	if (pod === undefined) return forward([]);
	const skip = `pij pod down ${pod} --skip-close-procedure`;
	// The show goes where the down goes: the same flags before `pod` (B347-R1-01).
	const before = args
		.slice(0, Math.max(0, args.length - rest.length - 1))
		.filter((w) => w !== "--json");
	const shown = io.exec(pijRs, [...before, "--json", "pod", "show", pod], io.cwd, io.env, false);
	let answer: unknown;
	try {
		answer = JSON.parse(shown.stdout);
	} catch {
		answer = undefined;
	}
	// A show that fails or answers nothing readable could hide a close procedure: refuse, never assume none (a brake).
	if (shown.status !== 0 || !isObj(answer) || answer.ok !== true || !isObj(answer.data)) {
		const why =
			isObj(answer) && typeof answer.meta === "string"
				? answer.meta
				: tail(shown.stderr || shown.stdout || `exit ${shown.status}`, 1);
		io.err(
			`refused: pod down ${pod}: pij could not read the pod to find its close procedure (${why}); nothing was closed.`,
		);
		io.err(`  go on without it: ${skip}`);
		return EXIT_REFUSED;
	}
	const close = answer.data.close_procedure;
	if (isObj(close) && typeof close.error === "string") {
		io.err(
			`refused: pod down ${pod}: its template cannot be read (${close.error}), so its close procedure is unknown; nothing was closed.`,
		);
		io.err(`  restore the template, or go on without it: ${skip}`);
		return EXIT_REFUSED;
	}
	if (!isObj(close) || typeof close.id !== "string") return forward([]);
	const id = close.id;
	// A shipped template has no folder (null): its procedures are the repo's or the skill's.
	const folder = str(close.template);
	const where =
		folder === undefined ? "this repo nor the pij skill" : `${folder}/procedures nor this repo`;
	const templateFlag = folder === undefined ? "" : ` --template ${folder}`;
	const key = str(close.key) ?? `pod-${pod}`;
	const runIo: Io = args.includes("--json") ? { ...io, out: io.err } : io;
	const ctx = context(runIo, folder === undefined ? options : { ...options, template: folder });
	if (typeof ctx === "string") {
		io.err(
			`refused: pod down ${pod}: its close procedure ${id} cannot run: ${ctx}; nothing was closed.`,
		);
		io.err(`  go on without it: ${skip}`);
		return EXIT_REFUSED;
	}
	const procedure = findProcedure(ctx.loaded.procedures, id);
	if (procedure === undefined || Array.isArray(procedure)) {
		io.err(
			`refused: pod down ${pod}: its template names close procedure ${id}, which is in neither ${where}; nothing was closed.`,
		);
		io.err(`  see what it names: ${ctx.entry} list${templateFlag}`);
		io.err(`  go on without it: ${skip}`);
		return EXIT_REFUSED;
	}
	io.err(`pod down ${pod}: running the template's close procedure ${id} first (key ${key})`);
	// A step recorded done is accepted only while its done check still passes (B347-R1-02); nothing is replayed.
	const stale = staleStep(ctx, procedure, key);
	if (stale !== undefined) {
		io.err(
			`refused: pod down ${pod}: close procedure ${id} step ${stale.id} (${stale.title}) was done, and its check fails now; nothing was closed.`,
		);
		io.err(`  redo it: ${ctx.entry} ${id} ${stale.id} --again${templateFlag} --key ${key}`);
		io.err(`  then: pij pod down ${pod}   (or go on without it: ${skip})`);
		return EXIT_REFUSED;
	}
	const run = runThrough(ctx, procedure, key);
	if (!run.done) {
		io.err(
			`refused: pod down ${pod}: close procedure ${id} stopped${run.stoppedAt ? ` at step ${run.stoppedAt}` : ""}; nothing was closed.`,
		);
		io.err(
			`  resume: ${ctx.entry} ${id}${run.stoppedAt ? ` ${run.stoppedAt}` : ""}${templateFlag} --key ${key}`,
		);
		io.err(`  then: pij pod down ${pod}   (or go on without it: ${skip})`);
		return run.code === EXIT_OK ? EXIT_REFUSED : run.code;
	}
	const status = forward(["--close-procedure-ran"]);
	// Taken down: the next down of a pod by this name runs its close procedure afresh.
	if (status === EXIT_OK) clearRun(ctx.root, procedure.id, key);
	return status;
}

/** The first run step `key` records done whose live done check fails now; decisions and marks are kept as recorded. */
function staleStep(ctx: Ctx, procedure: Procedure, key: string): Step | undefined {
	const progress = readProgress(ctx.root);
	const parsed = parseFlags([]);
	const inputs = parsed.ok ? gatherInputs(ctx, procedure, parsed.flags) : "no flags";
	if (typeof inputs === "string") return undefined;
	for (const step of procedure.steps) {
		const record = recordOf(progress, procedure.id, key, step.id);
		if (!record || step.kind !== "run" || !step.done) continue;
		const check = runRecipe(
			ctx,
			procedure,
			step.done,
			stepEnv(ctx, procedure, step.id, key, inputs),
			false,
		);
		if (check.status !== 0) return step;
	}
	return undefined;
}

export function realIo(): Io {
	// A reader that closes early (`pij proc list | head -1`) is not an error.
	for (const stream of [process.stdout, process.stderr]) {
		stream.on("error", (error: NodeJS.ErrnoException) => {
			if (error.code !== "EPIPE") throw error;
		});
	}
	return {
		cwd: process.cwd(),
		env: process.env,
		out: (line) => process.stdout.write(`${line}\n`),
		err: (line) => process.stderr.write(`${line}\n`),
		exec: (file, args, cwd, env, inherit, timeoutMs) => {
			const result = spawnSync(file, args, {
				cwd,
				env,
				encoding: "utf8",
				stdio: inherit ? "inherit" : "pipe",
				timeout: timeoutMs,
			});
			if ((result.error as NodeJS.ErrnoException | undefined)?.code === "ETIMEDOUT")
				return {
					status: EXIT_TIMED_OUT,
					stdout: typeof result.stdout === "string" ? result.stdout : "",
					stderr: `timed out after ${Math.round((timeoutMs ?? 0) / 1000)} s`,
				};
			return {
				status: result.status ?? (result.error ? 127 : 1),
				stdout: typeof result.stdout === "string" ? result.stdout : "",
				stderr:
					typeof result.stderr === "string"
						? result.stderr
						: result.error
							? String(result.error)
							: "",
			};
		},
		now: () => new Date(),
	};
}

function invokedDirectly(): boolean {
	const script = process.argv[1];
	if (!script) return false;
	try {
		return realpathSync(script) === realpathSync(RUNNER_FILE);
	} catch {
		return false;
	}
}

if (invokedDirectly()) process.exitCode = main(process.argv.slice(2), realIo());
