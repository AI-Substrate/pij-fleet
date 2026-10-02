/** The shim has one authority: an explicit rs route or a named refusal.
 * Business grammar and caller authorization remain in the daemon. */
import { COPILOT_CONTROL_REFUSAL, validateCommand } from "./commands.js";
import { COLD_WAKE_CODE, PIJ_ENVELOPE_VERSION } from "./daemon-wire.js";
import { renderRosterTable } from "./roster-table.js";

interface RouteIdentity {
	readonly verb: string;
	readonly leaf?: string;
	readonly why: string;
}

export interface RsHttpRoute extends RouteIdentity {
	readonly rsPath: string;
	readonly method: "GET" | "POST";
	/** Existing daemon query names; null accepts a literal value, arrays constrain the declared grammar. */
	readonly query?: Readonly<Record<string, readonly string[] | null>>;
	/** Query parameters the shim always sends, never taken from argv. */
	readonly fixedQuery?: Readonly<Record<string, string>>;
	readonly render?: (payload: unknown) => string;
	readonly acknowledgePath?: string;
	readonly readsBodyFile?: boolean;
}

export interface RsNativeRoute extends RouteIdentity {
	/** Native stdout/stderr/exit are the contract, not a JSON envelope. */
	readonly nativeCommand: "commit-trailers" | "fleet-report";
}

export interface RsUnportedRoute extends RouteIdentity {
	readonly unported: string;
}

export type RsRouteRow = RsHttpRoute | RsNativeRoute | RsUnportedRoute;
export const RS_UNSUPPORTED_STATUS = "docs/how/pij-rs-api.md#unsupported-status";

function field(payload: unknown, name: string): unknown {
	return (payload as Record<string, unknown> | null | undefined)?.[name];
}

function text(payload: unknown, name: string): string | undefined {
	const value = field(payload, name);
	return typeof value === "string" && value !== "" ? value : undefined;
}

function renderAdopt(payload: unknown): string {
	const id = text(payload, "id") ?? "?";
	const pane = text(payload, "pane");
	const proc = field(payload, "proc") as { readonly pid?: number } | undefined;
	const where = pane === undefined ? "no pane" : `pane ${pane}`;
	const bound =
		proc?.pid === undefined
			? "not bound to a process"
			: `bound by process identity (pid ${proc.pid}), which rs observes itself`;
	return `adopted ${id} (${where}, ${bound}) — peers can now: pij send ${id} "<text>"`;
}

function renderRegister(payload: unknown): string {
	const harness = text(payload, "harness");
	if (harness === "pi" || harness === "omp")
		return `${harness === "omp" ? "OMP" : "Pi"} seats self-register at boot; you are already reachable — run \`pij whoami\` to confirm, never adopt`;
	return `registered ${text(payload, "id") ?? "?"} (${harness ?? "?"}, ${text(payload, "pane") === undefined ? "paneless pull" : "pane push"})`;
}

function renderWhoami(payload: unknown): string {
	return [
		`pij session: ${text(payload, "id") ?? "?"}`,
		`folder:      ${text(payload, "folder") ?? "?"}`,
		`state:       ${text(payload, "state") ?? "idle"}`,
		`role:        ${text(payload, "role") ?? "—"}`,
		`extension: ${text(payload, "extension_build") ?? "unknown"} (${text(payload, "extension_path") ?? "unknown"})`,
	].join("\n");
}

function renderSessions(payload: unknown): string {
	const rows = field(payload, "rows");
	if (!Array.isArray(rows) || rows.length === 0) return "no pij sessions";
	const cell = (value: unknown): string =>
		value === undefined || value === null || value === "" ? "—" : String(value);
	return [
		"pij-id  generation  harness  harness-session  lifecycle  model  parent  transcript",
		...rows.map((row) =>
			[
				"pijId",
				"generation",
				"harness",
				"harnessSessionId",
				"lifecycle",
				"boundModel",
				"spawnedBy",
				"transcriptPath",
			]
				.map((key) => cell(field(row, key)))
				.join("  "),
		),
		`${rows.length} session(s)`,
	].join("\n");
}

function renderPhonehome(payload: unknown): string {
	const seat = text(payload, "seat") ?? "?";
	const harness = text(payload, "harness") ?? "?";
	if (field(payload, "bound") === true)
		return `phoned home: ${seat} ↔ ${harness} (bound by process identity, observed by the daemon)`;
	const why = text(payload, "resolved_by");
	return `phoned home: ${seat} — NOT bound${why === undefined ? "" : ` (${why})`}; rs binds by (pid, proc_start) and could not observe that pair`;
}

function renderState(payload: unknown): string {
	const head = `${text(payload, "id") ?? "?"}: ${text(payload, "state") ?? "?"} · ${text(payload, "liveness") ?? "?"}`;
	const detail = ["cwd", "harness", "parent", "native_receiver_reason"].flatMap((key) => {
		const value = text(payload, key);
		return value === undefined ? [] : [`${key}: ${value}`];
	});
	const deferrals = field(payload, "deliveryDeferrals");
	const deferred = Array.isArray(deferrals)
		? deferrals.map(
				(item) =>
					`delivery deferred: ${text(item, "reason") ?? "?"} ×${String(field(item, "count"))} since ${String(field(item, "since_ms"))} ms`,
			)
		: [];
	// Plan 160: the daemon renders the size lines once; print them verbatim.
	const sizeLines = field(payload, "sizeLines");
	const sized = Array.isArray(sizeLines)
		? sizeLines.filter((line) => typeof line === "string")
		: [];
	return `${head}${detail.length === 0 ? "" : `\n  ${detail.join("  ·  ")}`}${deferred.length === 0 ? "" : `\n${deferred.join("\n")}`}${sized.length === 0 ? "" : `\n${sized.join("\n")}`}`;
}

function renderShimSend(payload: unknown): string {
	const outcome = field(payload, "outcome");
	const word =
		typeof outcome === "object" && outcome !== null
			? (field(outcome, "outcome") ?? "?")
			: (outcome ?? "?");
	const reason = text(outcome, "reason");
	const msgId = text(payload, "msg_id") ?? "?";
	if (word === "held" && reason === "fyi") {
		const warning = text(payload, "warning");
		return `pij send: held (fyi) — ${msgId}${warning === undefined ? "" : `\n${warning}`}`;
	}
	const coldCheck = text(payload, "cold_check");
	return `sent (rs) — msg ${msgId} — receipt ${String(word)}${reason === undefined ? "" : ` — ${reason}`}${coldCheck === undefined ? "" : ` — cold-check: ${coldCheck}`}`;
}

function renderShimInbox(payload: unknown): string {
	if (!Array.isArray(payload) || payload.length === 0) return "inbox: empty";
	return [
		`inbox: ${payload.length} claimed`,
		...payload.map((claim) => {
			const message = field(claim, "message");
			return `  from ${text(message, "from") ?? "?"}: ${text(message, "body") ?? ""}`;
		}),
	].join("\n");
}

/** The actual skill verb table is checked against this explicit inventory.
 * An unknown verb is still refused, but it is NOT silently considered covered. */
export const RS_ROUTE_TABLE: readonly RsRouteRow[] = [
	{
		verb: "adopt",
		rsPath: "/v1/adopt",
		method: "POST",
		render: renderAdopt,
		why: "Daemon admission derives identity and role",
	},
	{
		verb: "whoami",
		rsPath: "/v1/whoami",
		method: "POST",
		render: renderWhoami,
		why: "Resolve the caller in rs",
	},
	{
		verb: "phonehome",
		rsPath: "/v1/phonehome",
		method: "POST",
		render: renderPhonehome,
		why: "Read the admitted binding",
	},
	{
		verb: "state",
		rsPath: "/v1/state",
		method: "POST",
		render: renderState,
		why: "Read the rs state card",
	},
	{
		verb: "list",
		rsPath: "/v1/seats",
		method: "GET",
		query: { harness: null, folder: null, parent: null, scope: ["local"], here: ["true", "false"] },
		fixedQuery: { sizes: "true" },
		render: renderRosterTable,
		why: "Read rs seats using the existing server query filters, with each seat's size (plan 160)",
	},
	{
		verb: "sessions",
		rsPath: "/v1/shim/sessions",
		method: "GET",
		render: renderSessions,
		why: "Read rs session rows without a legacy union",
	},
	{
		verb: "send",
		rsPath: "/v1/shim/send",
		method: "POST",
		readsBodyFile: true,
		render: renderShimSend,
		why: "Existing daemon message/control admission",
	},
	{
		verb: "compact-self",
		rsPath: "/v1/shim/compact-self",
		method: "POST",
		render: renderShimSend,
		why: "Existing daemon-owned self control",
	},
	{
		verb: "inbox",
		rsPath: "/v1/shim/inbox",
		method: "POST",
		acknowledgePath: "/v1/shim/inbox/ack",
		render: renderShimInbox,
		why: "Render claimed messages before acknowledging",
	},
	{
		verb: "inbox",
		leaf: "check",
		rsPath: "/v1/shim/inbox",
		method: "POST",
		acknowledgePath: "/v1/shim/inbox/ack",
		render: renderShimInbox,
		why: "Explicit alias of the same claim path",
	},
	{
		verb: "inbox",
		leaf: "register",
		rsPath: "/v1/register",
		method: "POST",
		render: renderRegister,
		why: "Resolve registered panes or admit verified external pull hosts through native rs registration",
	},
	{
		verb: "bg",
		rsPath: "/v1/bg",
		method: "POST",
		why: "Preserve shipped background jobs and their sole daemon parser",
	},
	...[
		"project",
		"stream",
		"fence",
		"dispatch",
		"ack",
		"canary",
		"attest",
		"task",
		"node",
		"orchestration",
		"spine",
		"role",
		"report",
		"close",
		"reap",
		"anomalies",
		"decisions",
		"answer",
	].map(
		(verb): RsHttpRoute => ({
			verb,
			rsPath: `/v1/${verb}`,
			method: "POST",
			why: "Forward argv and caller to the existing shared daemon operation",
		}),
	),
	{
		verb: "commit-trailers",
		nativeCommand: "commit-trailers",
		why: "Forward the native trailer-only stdout, stderr and exit contract",
	},
	{ verb: "fleet-report", nativeCommand: "fleet-report", why: "Forward the native report writer" },
	{
		verb: "spawn",
		unported: "shim spawn grammar is not native spawn grammar; use pij-rs spawn --help",
		why: "Do not silently discard layout/task/branch options",
	},
	{
		verb: "revive",
		unported: "shim revive grammar is not native revive grammar; use pij-rs revive --help",
		why: "Do not rename attach/print semantics",
	},
	{
		verb: "tail",
		unported: "peer transcript tail is not the native daemon event tail",
		why: "Different objects are not aliases",
	},
	...[
		"daemon",
		"path",
		"telegram",
		"agent",
		"watch",
		"unwatch",
		"chore",
		"watchdog",
		"focus",
		"tree",
		"link",
		"models",
	].map(
		(verb): RsUnportedRoute => ({
			verb,
			unported: "not ported through this shim",
			why: "No second live store or implicit legacy execution",
		}),
	),
];

export const LEAF_CLOSED_VERBS: readonly string[] = ["inbox"];

export function findRoute(
	verb: string,
	leaf: string | undefined,
	table: readonly RsRouteRow[] = RS_ROUTE_TABLE,
	leafClosed: readonly string[] = LEAF_CLOSED_VERBS,
): RsRouteRow | undefined {
	if (leafClosed.includes(verb)) return table.find((row) => row.verb === verb && row.leaf === leaf);
	if (leaf !== undefined) {
		const exact = table.find((row) => row.verb === verb && row.leaf === leaf);
		if (exact !== undefined) return exact;
	}
	return table.find((row) => row.verb === verb && row.leaf === undefined);
}

export function copilotControlRefusal(
	harness: string | undefined,
	command: string,
): string | undefined {
	const validation = validateCommand(command);
	return validation.ok && harness === "copilot" ? COPILOT_CONTROL_REFUSAL : undefined;
}

/** Wire names are frozen; omitted evidence is not invented or defaulted. */
export interface CallerContext {
	readonly pijSessionId?: string;
	readonly tmuxPane?: string;
	readonly pijParentId?: string;
	readonly claudeCodeSessionId?: string;
	readonly copilotAgentSessionId?: string;
	readonly codexThreadId?: string;
	readonly cwd?: string;
	readonly pid?: number;
	readonly procStart?: number;
}

export const CALLER_ENV_ALLOWLIST = [
	["PIJ_SESSION_ID", "pijSessionId"],
	["TMUX_PANE", "tmuxPane"],
	["PIJ_PARENT_ID", "pijParentId"],
	["CLAUDE_CODE_SESSION_ID", "claudeCodeSessionId"],
	["COPILOT_AGENT_SESSION_ID", "copilotAgentSessionId"],
	["CODEX_THREAD_ID", "codexThreadId"],
] as const satisfies readonly (readonly [string, keyof CallerContext])[];

export const CALLER_WIRE_KEYS: readonly string[] = [
	...CALLER_ENV_ALLOWLIST.map(([, key]) => key as string),
	"cwd",
	"pid",
	"procStart",
];

export function buildCallerContext(
	env: NodeJS.ProcessEnv,
	process: { readonly cwd?: string; readonly pid?: number; readonly procStart?: number },
): CallerContext {
	const caller: Record<string, string | number> = {};
	for (const [envName, key] of CALLER_ENV_ALLOWLIST) {
		const value = env[envName];
		if (value !== undefined && value !== "") caller[key] = value;
	}
	if (process.cwd !== undefined) caller.cwd = process.cwd;
	if (process.pid !== undefined) caller.pid = process.pid;
	if (process.procStart !== undefined) caller.procStart = process.procStart;
	return caller as CallerContext;
}

export type GenerationForce = "rs" | "legacy" | undefined;
export function resolveGenerationForce(env: {
	readonly PIJ_DAEMON_GENERATION?: string | undefined;
}): GenerationForce {
	const value = env.PIJ_DAEMON_GENERATION;
	if (value === undefined || value === "") return undefined;
	if (value === "rs" || value === "legacy") return value;
	throw new Error(
		`PIJ_DAEMON_GENERATION has malformed value ${JSON.stringify(value)}: expected exactly "rs" or "legacy"`,
	);
}

export type RouteOutcome =
	| { readonly kind: "rs"; readonly verb: string; readonly addr: string; readonly forced: boolean }
	| {
			readonly kind: "rs-native";
			readonly verb: string;
			readonly addr: string;
			readonly exitCode: number;
	  }
	| {
			readonly kind: "rs-auth-rejected";
			readonly verb: string;
			readonly addr: string;
			readonly stateDir: string;
	  }
	| {
			readonly kind: "rs-unreadable";
			readonly verb: string;
			readonly addr: string;
			readonly detail: string;
	  }
	| {
			readonly kind: "rs-error";
			readonly verb: string;
			readonly addr: string;
			readonly detail: string;
			readonly code?: string;
	  };

export function unported(verb: string, addr: string, reason: string): RouteOutcome {
	return {
		kind: "rs-error",
		verb,
		addr,
		code: "E-RS-UNPORTED",
		detail: `E-RS-UNPORTED ${verb}: ${reason}; see ${RS_UNSUPPORTED_STATUS}`,
	};
}

export function describeOutcome(outcome: RouteOutcome): string {
	switch (outcome.kind) {
		case "rs":
			return `${outcome.verb}: served by rs at ${outcome.addr}${outcome.forced ? " (forced: PIJ_DAEMON_GENERATION=rs)" : ""}`;
		case "rs-native":
			return `${outcome.verb}: native pij-rs exited ${outcome.exitCode}`;
		case "rs-auth-rejected":
			return `${outcome.verb}: rs at ${outcome.addr} rejected the key at ${outcome.stateDir}/daemon.key; no legacy fallback`;
		case "rs-unreadable":
			return `${outcome.verb}: unreadable rs response at ${outcome.addr}: ${outcome.detail}`;
		case "rs-error":
			return `${outcome.detail} (rs at ${outcome.addr})`;
	}
}

/** The human stderr line for a refusal. A cold-wake refusal is the daemon's
 *  whole answer — the price and both ways forward — so it prints alone. */
export function refusalText(outcome: RouteOutcome): string {
	if (outcome.kind === "rs-error" && outcome.detail.startsWith(`${COLD_WAKE_CODE}:`))
		return outcome.detail;
	return `E-RS: ${describeOutcome(outcome)}`;
}

/** Locally detected refusals use the same v2 shape; a daemon envelope is NEVER rebuilt. */
export function routingRefusalEnvelope(outcome: RouteOutcome): string {
	const code =
		outcome.kind === "rs-error"
			? (outcome.code ?? "E-RS-REQUEST")
			: outcome.kind === "rs-auth-rejected"
				? "E-RS-AUTH"
				: "E-RS-WIRE";
	return JSON.stringify({
		ok: false,
		command: `pij ${outcome.verb}`,
		v: PIJ_ENVELOPE_VERSION,
		error: outcome.kind === "rs-auth-rejected" ? "auth" : "refused",
		meta: describeOutcome(outcome),
		details: { code, verb: outcome.verb, ledger_item: RS_UNSUPPORTED_STATUS },
	});
}

export interface PreCallInput {
	readonly verb: string;
	readonly leaf?: string | undefined;
	readonly force: GenerationForce;
	readonly addr: string;
	readonly rsLive: boolean;
}

export function decidePreCall(
	input: PreCallInput,
	table: readonly RsRouteRow[] = RS_ROUTE_TABLE,
): { readonly kind: "try-rs"; readonly row: RsHttpRoute | RsNativeRoute } | RouteOutcome {
	const { verb, addr, force } = input;
	if (force === "legacy")
		return unported(verb, addr, "PIJ_DAEMON_GENERATION=legacy is retired; unset it to use rs");
	const row = findRoute(verb, input.leaf, table);
	if (row === undefined)
		return unported(
			verb,
			addr,
			input.leaf === undefined ? "unknown verb" : `unknown command ${verb} ${input.leaf}`,
		);
	if ("unported" in row) return unported(verb, addr, row.unported);
	if ("rsPath" in row && !input.rsLive)
		return unported(verb, addr, `no rs daemon answered at ${addr}`);
	return { kind: "try-rs", row };
}

export interface RsResponseFacts {
	readonly status: number;
	readonly envelopeDecoded: boolean;
	readonly refused?: boolean;
	readonly detail?: string | undefined;
}

export function classifyRsResponse(
	verb: string,
	addr: string,
	row: RsHttpRoute,
	facts: RsResponseFacts,
	forced: boolean,
	stateDir: string,
): RouteOutcome {
	if (facts.status === 401 || (facts.status === 403 && !facts.envelopeDecoded)) {
		return { kind: "rs-auth-rejected", verb, addr, stateDir };
	}
	if (!facts.envelopeDecoded) {
		if (facts.status === 404 || facts.status === 405)
			return unported(verb, addr, `rs does not serve ${row.rsPath}`);
		return {
			kind: "rs-unreadable",
			verb,
			addr,
			detail: facts.detail ?? `HTTP ${facts.status} without a valid v2 envelope`,
		};
	}
	if (facts.refused || facts.status >= 400)
		return {
			kind: "rs-error",
			verb,
			addr,
			detail: facts.detail ?? `HTTP ${facts.status} from ${row.rsPath}`,
		};
	return { kind: "rs", verb, addr, forced };
}

export type RsRenderResult =
	| { readonly kind: "json" | "rendered"; readonly text: string }
	| { readonly kind: "no-renderer" | "json-refused"; readonly message: string };

export function renderRsAnswer(
	row: RsRouteRow | undefined,
	payload: unknown,
	argv: readonly string[],
	rawEnvelope?: string,
): RsRenderResult {
	if (argv.includes("--json")) {
		return rawEnvelope === undefined
			? { kind: "json-refused", message: "no validated original v2 envelope is available" }
			: { kind: "json", text: rawEnvelope };
	}
	if (row === undefined || !("rsPath" in row))
		return { kind: "no-renderer", message: "no HTTP response renderer for this invocation" };
	if (row.render !== undefined) return { kind: "rendered", text: row.render(payload) };
	const line = text(payload, "line");
	return { kind: "rendered", text: line ?? JSON.stringify(payload, null, 2) };
}
