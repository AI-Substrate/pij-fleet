// pij-control-plane — pure model-registry read (T002).
//
// Pi-first: parseModelsJson covers the live ~/.pi/agent/models.json shape.
// copilotSeedFromPi seeds from pi's github-copilot provider section.
// claudeAliases + codexSnapshot are honest best-effort/unverified fallbacks.
// The pure parsers take already-read text/JSON; `loadModels()` is the single
// impure composition root that reads pi's models.json + codex's config.toml off
// disk and merges every source (moved here from the bin in plan 029 T002 so the
// `pij agent` CLI surface can reuse the exact same registry without importing the
// bin). All parsing stays pure and separately testable.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export type ModelRuntime = "pi" | "omp" | "copilot" | "claude" | "codex";

export interface ModelEntry {
	/** Runtime-local model id. OMP `-1m` aliases intentionally remain distinct ids. */
	readonly id: string;
	readonly name: string;
	/** Upstream provider, separate from the runtime that accepts the selector. */
	readonly provider: string;
	/** Target runtime/harness for this row. Optional only for legacy injected test fixtures. */
	readonly runtime?: ModelRuntime;
	/** Exact selector a caller should pass to the target runtime. */
	readonly selector?: string;
	/** Upstream request id when a runtime selector is an alias (for example OMP `-1m`). */
	readonly requestModelId?: string;
	/** false = best-effort alias list (claude/codex) — not confirmed by a live registry. */
	readonly verified: boolean;
	/** Does the model support a thinking/reasoning effort level? */
	readonly reasoning?: boolean;
	/** Canonical effort levels honored by this runtime selector. */
	readonly levels?: readonly string[];
	/** Context-window capacity in tokens; absent when the source has none. */
	readonly contextWindow?: number;
}

interface PiModel {
	readonly id?: unknown;
	readonly name?: unknown;
	readonly reasoning?: unknown;
	readonly thinkingLevelMap?: unknown;
	readonly contextWindow?: unknown;
}

/** A usable window is a finite POSITIVE token count; anything else is honest
 *  absence (T007 — the gauge law tolerates no bogus capacities). */
function usableContextWindow(value: unknown): number | undefined {
	return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;
}

interface PiProvider {
	readonly models?: ReadonlyArray<PiModel>;
	readonly modelOverrides?: Readonly<Record<string, unknown>>;
}

interface PiModelsJson {
	readonly providers?: Readonly<Record<string, PiProvider>>;
}

function isObj(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null;
}

/** Canonical effort levels a model honors = the NON-NULL keys of pi's
 *  `thinkingLevelMap` ({canonical → native|null}; null = unsupported for that
 *  model, so it drops out). Returns [] when there's no usable map (#1). */
function levelsFromThinkingMap(map: unknown): string[] {
	if (!isObj(map)) return [];
	return Object.entries(map)
		.filter(([, v]) => v !== null && v !== undefined)
		.map(([k]) => k);
}

function nonEmptyString(value: unknown): value is string {
	return typeof value === "string" && value.trim().length > 0;
}

/** OMP reports canonical effort names as an array; null explicitly means no levels. */
function levelsFromOmpThinking(thinking: unknown): string[] | undefined {
	if (thinking === undefined || thinking === null) return [];
	if (
		!Array.isArray(thinking) ||
		!thinking.every((level) => nonEmptyString(level) && level.trim() === level)
	) {
		return undefined;
	}
	return [...thinking];
}

const COPILOT_GPT56_LEVELS = ["none", "low", "medium", "high", "xhigh", "max"];
const COPILOT_GPT56_IDS = new Set([
	"gpt-5.6-sol",
	"gpt-5.6-sol-fast",
	"gpt-5.6-terra",
	"gpt-5.6-luna",
]);

function isCopilotGpt56(id: string): boolean {
	return COPILOT_GPT56_IDS.has(id.replace(/-1m$/, ""));
}

const OMP_GPT56_CONTEXTS: Readonly<Record<string, number>> = {
	"gpt-5.6-luna": 328_000,
	"gpt-5.6-sol": 400_000,
	"gpt-5.6-sol-fast": 400_000,
	"gpt-5.6-terra": 400_000,
	"gpt-5.6-luna-1m": 1_050_000,
	"gpt-5.6-sol-1m": 1_050_000,
	"gpt-5.6-sol-fast-1m": 1_050_000,
	"gpt-5.6-terra-1m": 1_050_000,
};

function ompContextWindow(provider: string, id: string, reported: unknown): number | undefined {
	if (provider === "github-copilot") {
		const measured = OMP_GPT56_CONTEXTS[id];
		if (measured !== undefined) return measured;
	}
	return usableContextWindow(reported);
}

function piModelLevels(provider: string, id: string, thinkingLevelMap: unknown): string[] {
	if (provider === "github-copilot" && isCopilotGpt56(id)) {
		return [...COPILOT_GPT56_LEVELS];
	}
	return levelsFromThinkingMap(thinkingLevelMap);
}

/**
 * Parse `~/.pi/agent/models.json` (the live pi model registry). Returns one
 * `ModelEntry` per model/override across all providers. Pure: the caller reads
 * the file and passes the parsed JSON.
 *
 * Each provider's `models[]` is primary; `modelOverrides` (provider-level
 * renames / additions) are included unless the id is already in `models[]`.
 */
export function parseModelsJson(raw: unknown): ModelEntry[] {
	if (!isObj(raw)) return [];
	const json = raw as PiModelsJson;
	if (!isObj(json.providers)) return [];
	const entries: ModelEntry[] = [];
	for (const [provider, data] of Object.entries(json.providers)) {
		if (!isObj(data)) continue;
		const seenIds = new Set<string>();
		if (Array.isArray(data.models)) {
			for (const m of data.models) {
				if (!isObj(m) || typeof m.id !== "string") continue;
				const id: string = m.id;
				seenIds.add(id);
				const window = usableContextWindow(m.contextWindow);
				entries.push({
					id,
					name: typeof m.name === "string" ? m.name : id,
					provider,
					runtime: "pi",
					selector: `${provider}/${id}`,
					requestModelId: id,
					verified: true,
					reasoning:
						isCopilotGpt56(id) && provider === "github-copilot" ? true : m.reasoning === true,
					levels: piModelLevels(provider, id, m.thinkingLevelMap),
					...(window === undefined ? {} : { contextWindow: window }),
				});
			}
		}
		if (isObj(data.modelOverrides)) {
			for (const [id, override] of Object.entries(data.modelOverrides as Record<string, unknown>)) {
				if (seenIds.has(id)) continue; // don't duplicate
				const ov = isObj(override) ? override : {};
				const name = typeof ov.name === "string" ? ov.name : id;
				const window = usableContextWindow(ov.contextWindow);
				entries.push({
					id,
					name,
					provider,
					runtime: "pi",
					selector: `${provider}/${id}`,
					requestModelId: id,
					verified: true,
					reasoning:
						isCopilotGpt56(id) && provider === "github-copilot" ? true : ov.reasoning === true,
					levels: piModelLevels(provider, id, ov.thinkingLevelMap),
					...(window === undefined ? {} : { contextWindow: window }),
				});
			}
		}
	}
	return entries;
}

/** Seed Copilot CLI models from pi's `github-copilot` provider section. */
export function copilotSeedFromPi(raw: unknown): ModelEntry[] {
	if (!isObj(raw)) return [];
	const json = raw as PiModelsJson;
	if (!isObj(json.providers)) return [];
	const section = (json.providers as Record<string, unknown>)["github-copilot"];
	if (!isObj(section)) return [];
	const parsed = parseModelsJson({ providers: { "github-copilot": section } });
	return parsed.map((entry) => ({
		...entry,
		provider: "github-copilot",
		runtime: "copilot",
		selector: entry.id,
		requestModelId: entry.id,
		...(isCopilotGpt56(entry.id) ? { contextWindow: 1_050_000 } : {}),
	}));
}

/** Known Claude CLI model aliases (best-effort, unverified — not from a live API). */
export function claudeAliases(): ModelEntry[] {
	const aliases = [
		["claude-opus-5", "Claude Opus 5"],
		["claude-fable-5", "Claude Fable 5"],
		["claude-sonnet-5", "Claude Sonnet 5"],
		["claude-opus-4-8", "Claude Opus 4.8"],
		["claude-sonnet-4-6", "Claude Sonnet 4.6"],
		["claude-haiku-4-5-20251001", "Claude Haiku 4.5"],
		["claude-opus-4-5", "Claude Opus 4.5"],
		["claude-sonnet-4-5", "Claude Sonnet 4.5"],
		["claude-haiku-4-5", "Claude Haiku 4.5 (legacy)"],
	] as const;
	return aliases.map(([id, name]) => ({
		id,
		name,
		provider: "claude",
		runtime: "claude",
		selector: id,
		requestModelId: id,
		verified: false,
	}));
}

// ── codex thinking levels (#2 — curated; not CLI-discoverable) ────────────────
// Codex has NO `--effort` flag and silently ignores bogus reasoning values, so the
// per-model honored levels can only come from a curated table (memory:
// thinking-level-discovery). gpt-5 family honors minimal→xhigh; o-series stops at high.
const CODEX_GPT5_LEVELS = ["minimal", "low", "medium", "high", "xhigh"];
const CODEX_OSERIES_LEVELS = ["minimal", "low", "medium", "high"];

/** Curated reasoning levels for a codex model id (empty when unknown). */
function codexLevelsFor(id: string): string[] {
	if (/^gpt-5/i.test(id)) return [...CODEX_GPT5_LEVELS];
	if (/^o\d/i.test(id)) return [...CODEX_OSERIES_LEVELS];
	return [];
}

/** Build a codex ModelEntry (best-effort/unverified) with curated levels. */
function codexEntry(id: string, name?: string): ModelEntry {
	const levels = codexLevelsFor(id);
	return {
		id,
		name: name ?? id,
		provider: "codex",
		runtime: "codex",
		selector: id,
		requestModelId: id,
		verified: false,
		reasoning: levels.length > 0,
		levels,
	};
}

/** Thin fallback of known Codex model ids (best-effort, unverified). The PRIMARY
 *  source is now the user's `~/.codex/config.toml` default model (see
 *  {@link codexConfigModels} / cli.ts loadModels); this just keeps `pij models`
 *  non-empty for codex when no config is readable. */
export function codexSnapshot(): ModelEntry[] {
	return [
		// gpt-5.6 trio (sol/terra/luna) — served by the codex client but not
		// CLI-enumerable, so they only appear here as best-effort aliases (the
		// config default still wins via loadModels dedup when set).
		codexEntry("gpt-5.6-sol"),
		codexEntry("gpt-5.6-terra"),
		codexEntry("gpt-5.6-luna"),
		codexEntry("gpt-5.5"),
		codexEntry("o3"),
	];
}

/** Build a measured Copilot CLI fallback row. `-1m` aliases are deliberately absent. */
function copilotEntry(id: string, name?: string): ModelEntry {
	const levels = isCopilotGpt56(id) ? [...COPILOT_GPT56_LEVELS] : [];
	return {
		id,
		name: name ?? id,
		provider: "github-copilot",
		runtime: "copilot",
		selector: id,
		requestModelId: id,
		verified: false,
		reasoning: levels.length > 0,
		levels,
		...(isCopilotGpt56(id) ? { contextWindow: 1_050_000 } : {}),
	};
}

/** Actual Copilot CLI ids measured from entitlement/API; never OMP client aliases. */
export function copilotSnapshot(): ModelEntry[] {
	return [
		copilotEntry("gpt-5.6-sol"),
		copilotEntry("gpt-5.6-sol-fast"),
		copilotEntry("gpt-5.6-terra"),
		copilotEntry("gpt-5.6-luna"),
	];
}

interface OmpModel {
	readonly id?: unknown;
	readonly name?: unknown;
	readonly provider?: unknown;
	readonly selector?: unknown;
	readonly contextWindow?: unknown;
	readonly reasoning?: unknown;
	readonly thinking?: unknown;
}

interface OmpModelsJson {
	readonly models?: ReadonlyArray<OmpModel>;
}

function isValidOmpModel(model: unknown): model is OmpModel & {
	readonly id: string;
	readonly provider: string;
	readonly selector: string;
} {
	if (!isObj(model)) return false;
	if (!nonEmptyString(model.id) || !nonEmptyString(model.provider)) return false;
	if (!nonEmptyString(model.selector)) return false;
	if (model.name !== undefined && typeof model.name !== "string") return false;
	if (model.contextWindow !== undefined && usableContextWindow(model.contextWindow) === undefined) {
		return false;
	}
	if (model.reasoning !== undefined && typeof model.reasoning !== "boolean") return false;
	return levelsFromOmpThinking(model.thinking) !== undefined;
}

/** Parse the supported `omp models --json --no-extensions` inventory shape. */
export function parseOmpModelsJson(raw: unknown): ModelEntry[] {
	if (!isObj(raw)) return [];
	const json = raw as OmpModelsJson;
	if (!Array.isArray(json.models) || json.models.length === 0) return [];
	if (!json.models.every(isValidOmpModel)) return [];
	return json.models.map((model) => {
		const { id, provider, selector } = model;
		const requestModelId = provider === "github-copilot" ? id.replace(/-1m$/, "") : id;
		const sourceLevels = levelsFromOmpThinking(model.thinking) ?? [];
		const levels =
			provider === "github-copilot" && isCopilotGpt56(requestModelId)
				? [...COPILOT_GPT56_LEVELS]
				: sourceLevels;
		const window = ompContextWindow(provider, id, model.contextWindow);
		return {
			id,
			name: typeof model.name === "string" ? model.name : id,
			provider,
			runtime: "omp" as const,
			selector,
			requestModelId,
			verified: true,
			reasoning: levels.length > 0 || model.reasoning === true,
			levels,
			...(window === undefined ? {} : { contextWindow: window }),
		};
	});
}

export interface OmpCommandResult {
	readonly status: number | null;
	readonly stdout: string;
	readonly stderr: string;
}

export interface OmpCommandOptions {
	readonly timeout: number;
}

export type OmpCommandRunner = (
	command: string,
	args: readonly string[],
	options: OmpCommandOptions,
) => OmpCommandResult;

export const OMP_MODELS_TIMEOUT_MS = 5_000;

export type OmpModelsResult =
	| { readonly ok: true; readonly models: readonly ModelEntry[] }
	| { readonly ok: false; readonly error: string };

function runOmpCommand(
	command: string,
	args: readonly string[],
	options: OmpCommandOptions,
): OmpCommandResult {
	const result = spawnSync(command, [...args], {
		encoding: "utf8",
		stdio: "pipe",
		timeout: options.timeout,
	});
	return {
		status: result.status,
		stdout: result.stdout ?? "",
		stderr: result.error?.message ?? result.stderr ?? "",
	};
}

/** Load OMP's effective local inventory without refreshing or reading its cache internals. */
export function loadOmpModels(run: OmpCommandRunner = runOmpCommand): OmpModelsResult {
	const result = run("omp", ["models", "--json", "--no-extensions"], {
		timeout: OMP_MODELS_TIMEOUT_MS,
	});
	if (result.status !== 0) {
		return {
			ok: false,
			error: `OMP model inventory unavailable (${result.stderr.trim() || `exit ${String(result.status)}`})`,
		};
	}
	let raw: unknown;
	try {
		raw = JSON.parse(result.stdout);
	} catch (error: unknown) {
		const detail = error instanceof Error ? error.message : String(error);
		return { ok: false, error: `OMP model inventory is not valid JSON (${detail})` };
	}
	const models = parseOmpModelsJson(raw);
	if (models.length === 0) {
		return { ok: false, error: "OMP model inventory contained no usable models" };
	}
	return { ok: true, models };
}

function modelMatchesRuntime(entry: ModelEntry, runtime: ModelRuntime): boolean {
	if (entry.runtime !== undefined) return entry.runtime === runtime;
	if (runtime === "pi") {
		return (
			entry.provider !== "copilot" && entry.provider !== "claude" && entry.provider !== "codex"
		);
	}
	if (runtime === "copilot") {
		return entry.provider === "copilot" || entry.provider === "github-copilot";
	}
	return entry.provider === runtime;
}

/** Select rows for the target runtime, falling back only for legacy injected fixtures. */
export function modelsForRuntime(
	models: readonly ModelEntry[],
	runtime: ModelRuntime,
): ModelEntry[] {
	return models.filter((entry) => modelMatchesRuntime(entry, runtime));
}

const MODEL_RUNTIME_FILTERS = new Set<ModelRuntime>(["pi", "omp", "copilot", "claude", "codex"]);

/** A known runtime filter selects runtime rows; every other value is an upstream provider. */
export function modelsForFilter(models: readonly ModelEntry[], filter: string): ModelEntry[] {
	if (MODEL_RUNTIME_FILTERS.has(filter as ModelRuntime)) {
		return modelsForRuntime(models, filter as ModelRuntime);
	}
	return models.filter((entry) => entry.provider === filter);
}

export interface LoadedModelCatalog {
	readonly models: readonly ModelEntry[];
	readonly unavailableRuntimes: readonly ModelRuntime[];
}

/** Compose the normal registry and optionally the effective OMP inventory. */
export function loadModelCatalog(includeOmp = false): LoadedModelCatalog {
	const models = loadModels();
	if (!includeOmp) return { models, unavailableRuntimes: [] };
	const omp = loadOmpModels();
	return omp.ok
		? { models: [...models, ...omp.models], unavailableRuntimes: [] }
		: { models, unavailableRuntimes: ["omp"] };
}

/**
 * Parse the codex default model out of a `~/.codex/config.toml` text (#2). Codex
 * stores its active model as a TOP-LEVEL `model = "<id>"` key; a `model =` inside a
 * `[section]` (e.g. `[notice]`, `[projects.…]`) is unrelated and ignored. Returns a
 * single-entry list for that model (with curated levels), or [] when none/unreadable.
 * Pure — the caller reads the file. Minimal hand-parser (no TOML dependency).
 */
export function codexConfigModels(tomlText: string): ModelEntry[] {
	if (typeof tomlText !== "string") return [];
	let topLevel = true;
	for (const raw of tomlText.split("\n")) {
		const line = raw.trim();
		if (line === "" || line.startsWith("#")) continue;
		if (line.startsWith("[")) {
			topLevel = false; // entered a [section] — top-level keys are done
			continue;
		}
		if (!topLevel) continue;
		const m = line.match(/^model\s*=\s*["']([^"']+)["']/);
		if (m?.[1]) return [codexEntry(m[1])];
	}
	return [];
}

/**
 * Load + merge the full pij model registry from `~/.pi/agent/models.json` (pi +
 * copilot seed), claude aliases, and the codex default (`~/.codex/config.toml`)
 * plus its snapshot fallback. Best-effort: any I/O error degrades to the alias
 * lists so `pij models` / `pij agent` stay usable in CI / offline. Moved here
 * from the bin (plan 029 T002) — the composition is byte-for-byte the same, this
 * is the ONLY impure function in the module.
 */
export function loadModels(): ModelEntry[] {
	const piModelsPath = join(homedir(), ".pi", "agent", "models.json");
	let piRaw: unknown = null;
	try {
		piRaw = JSON.parse(readFileSync(piModelsPath, "utf8"));
	} catch {
		/* no pi install or no models.json → fall back to aliases only */
	}
	const piModels = parseModelsJson(piRaw);
	const copilotSeed = copilotSeedFromPi(piRaw);
	// Merge in the best-effort copilot snapshot (newer ids absent from pi's seed),
	// deduped by id so a VERIFIED pi entry always wins over an unverified alias.
	const copilotSeedIds = new Set(copilotSeed.map((m) => m.id));
	const copilotModels = [
		...copilotSeed,
		...copilotSnapshot().filter((m) => !copilotSeedIds.has(m.id)),
	];
	// claude, like codex below, is a DISTINCT harness target, so its aliases are NOT
	// deduped against pi/copilot: `claude-opus-5` on a Copilot subscription is a
	// different spawn path, auth and entitlement from the same id in Claude Code.
	// Cross-provider dedupe silently HID every shared id from the claude harness — an
	// operator reading `pij models` could not see that the model was launchable there.
	const claude = claudeAliases();
	// Codex (#2): prefer the user's configured default model from ~/.codex/config.toml
	// (best-effort; empty on any read/parse error), ahead of the thin static snapshot.
	let codexToml = "";
	try {
		codexToml = readFileSync(join(homedir(), ".codex", "config.toml"), "utf8");
	} catch {
		/* no codex install / unreadable config → snapshot-only fallback */
	}
	const codexConfig = codexConfigModels(codexToml);
	const codexCfgIds = new Set(codexConfig.map((m) => m.id));
	// codex is a DISTINCT harness target, so its entries are NOT deduped against
	// pi/copilot (a `gpt-5.5` under copilot ≠ the codex one — different harness,
	// different reasoning table). Dedup only WITHIN codex: the config default wins
	// over the thin snapshot fallback. (claude aliases stay seenIds-deduped above.)
	const codexFallback = codexSnapshot().filter((m) => !codexCfgIds.has(m.id));
	const codex = [...codexConfig, ...codexFallback];
	return [...piModels, ...copilotModels, ...claude, ...codex];
}
