/** I/O for the closed rs routing inventory. No legacy registry is consulted. */
import { spawn } from "node:child_process";
import {
	accessSync,
	constants as fsConstants,
	readdirSync,
	readFile as readFd,
	statSync,
} from "node:fs";
import { readFile as fsReadFile } from "node:fs/promises";
import { homedir, constants as osConstants } from "node:os";
import { join } from "node:path";
import {
	type AmbientNativeIdentity,
	resolveAmbientNativeIdentity,
} from "../core/current-session.js";
import {
	DaemonRefusalError,
	decodeEnvelope,
	PijNoDaemonError,
	PijWireSkewError,
	type Registration,
	type RustSeatDescriptor,
} from "../core/daemon-wire.js";
import {
	buildCallerContext,
	type CallerContext,
	classifyRsResponse,
	decidePreCall,
	describeOutcome,
	findRoute,
	type RouteOutcome,
	RS_ROUTE_TABLE,
	type RsHttpRoute,
	type RsRouteRow,
	resolveGenerationForce,
	routingRefusalEnvelope,
	unported,
} from "../core/generation-routing.js";
import {
	codexRolloutForSession,
	codexTranscriptRoot,
	listCodexRollouts,
} from "../core/harness/codex.js";
import { resolveCopilotCurrentSession } from "../core/harness/copilot.js";
import {
	type DaemonLocation,
	daemonLocation,
	detectDaemonGeneration,
	observedProcessStart,
} from "./daemon-http.js";

export interface GenerationRouterDeps {
	readonly fetch: typeof fetch;
	readonly readFile: typeof fsReadFile;
	readonly readBody: (path: string) => Promise<string>;
	readonly env: NodeJS.ProcessEnv;
	readonly home: string;
	readonly cwd?: (() => string) | undefined;
	readonly pid?: (() => number) | undefined;
	readonly procStart?: ((pid: number) => number) | undefined;
	/** Validated native session files, never a legacy registry or an inferred host PID. */
	readonly ambientIdentity?: (() => AmbientNativeIdentity | null) | undefined;
	/** Only the explicitly declared native row may call this; streams are inherited. */
	readonly runNative?: ((args: readonly string[]) => Promise<number>) | undefined;
}

export function daemonLocationForRouting(env: NodeJS.ProcessEnv, home: string): DaemonLocation {
	return daemonLocation(env, home);
}

export function defaultRouterDeps(): GenerationRouterDeps {
	return {
		fetch,
		readFile: fsReadFile,
		readBody: (path) =>
			path === "-"
				? new Promise((resolve, reject) =>
						readFd(0, "utf8", (error, body) => (error ? reject(error) : resolve(body))),
					)
				: fsReadFile(path, "utf8"),
		env: process.env,
		home: homedir(),
		cwd: () => process.cwd(),
		pid: () => process.pid,
		procStart: observedProcessStart,
		ambientIdentity: () => ambientIdentity(process.env, homedir()),
	};
}

function ambientIdentity(env: NodeJS.ProcessEnv, home: string): AmbientNativeIdentity | null {
	const readDir = (path: string): string[] => {
		try {
			return readdirSync(path);
		} catch {
			return [];
		}
	};
	let copilotCurrentSessionId: string | undefined;
	if (env.COPILOT_AGENT_SESSION_ID?.trim()) {
		const resolved = resolveCopilotCurrentSession(
			env.COPILOT_AGENT_SESSION_ID,
			(root) =>
				readDir(root).flatMap((name) => {
					try {
						const entry = statSync(join(root, name));
						return [{ name, mtimeMs: entry.mtimeMs, isDirectory: entry.isDirectory() }];
					} catch {
						return [];
					}
				}),
			home,
		);
		if (!resolved.ok) throw new Error(`E-AMBIG: ${resolved.message}`);
		copilotCurrentSessionId = resolved.sessionId;
	}
	let codexCurrentSession: { threadId: string; transcriptPath: string } | undefined;
	const threadId = env.CODEX_THREAD_ID?.trim().toLowerCase();
	if (threadId) {
		const transcriptPath = codexRolloutForSession(
			listCodexRollouts(readDir, codexTranscriptRoot(home)),
			threadId,
			(path) => {
				try {
					accessSync(path, fsConstants.R_OK);
					return statSync(path).isFile();
				} catch {
					return false;
				}
			},
		);
		if (transcriptPath === null)
			throw new Error("E-AMBIG: CODEX_THREAD_ID has no matching readable native rollout");
		codexCurrentSession = { threadId, transcriptPath };
	}
	const resolved = resolveAmbientNativeIdentity({
		...(env.CLAUDE_CODE_SESSION_ID === undefined
			? {}
			: { claudeCodeSessionId: env.CLAUDE_CODE_SESSION_ID }),
		...(copilotCurrentSessionId === undefined ? {} : { copilotCurrentSessionId }),
		...(codexCurrentSession === undefined ? {} : { codexCurrentSession }),
	});
	if (!resolved.ok) throw new Error(`${resolved.code}: ${resolved.message}`);
	if (threadId && resolved.value?.harness !== "codex")
		throw new Error("E-AMBIG: CODEX_THREAD_ID is not a valid native session id");
	return resolved.value;
}

export interface RouteResult {
	readonly outcome: RouteOutcome;
	readonly payload?: unknown;
	readonly row?: RsRouteRow;
	/** Exact validated HTTP response bytes, or a locally generated v2 refusal. */
	readonly rawEnvelope?: string;
	/** Invoke only after rendering/printing succeeds. */
	readonly acknowledge?: () => Promise<string | undefined>;
}

export type RsProbe = (
	| {
			readonly kind: "live" | "absent" | "auth-rejected" | "no-credentials";
			readonly location: DaemonLocation;
	  }
	| { readonly kind: "unreadable"; readonly location: DaemonLocation; readonly detail: string }
) & { readonly rawEnvelope?: string };

export async function probeRs(deps: GenerationRouterDeps): Promise<RsProbe> {
	const location = daemonLocation(deps.env, deps.home);
	try {
		const generation = await detectDaemonGeneration(location, "rs", {
			fetch: deps.fetch,
			readFile: deps.readFile,
			processStart: () => 0,
		});
		return { kind: generation.kind === "rust" ? "live" : "absent", location };
	} catch (error) {
		if (error instanceof PijNoDaemonError) return { kind: "absent", location };
		if (error instanceof PijWireSkewError)
			return { kind: "unreadable", location, detail: error.message };
		if (error instanceof DaemonRefusalError && error.kind === "auth") {
			let hasKey = false;
			try {
				hasKey = (await deps.readFile(join(location.stateDir, "daemon.key"), "utf8")).trim() !== "";
			} catch {
				/* Absence never authorizes another store. */
			}
			return {
				kind: hasKey ? "auth-rejected" : "no-credentials",
				location,
				...(error.rawEnvelope === undefined ? {} : { rawEnvelope: error.rawEnvelope }),
			};
		}
		return {
			kind: "unreadable",
			location,
			detail: error instanceof Error ? error.message : String(error),
			...(error instanceof DaemonRefusalError && error.rawEnvelope !== undefined
				? { rawEnvelope: error.rawEnvelope }
				: {}),
		};
	}
}

function callerProcessFacts(deps: GenerationRouterDeps): {
	readonly cwd?: string;
	readonly pid?: number;
	readonly procStart?: number;
} {
	const pid = deps.pid?.();
	let procStart: number | undefined;
	if (pid !== undefined && deps.procStart !== undefined) {
		try {
			procStart = deps.procStart(pid);
		} catch {
			/* Do not invent an incarnation. */
		}
	}
	return {
		...(deps.cwd === undefined ? {} : { cwd: deps.cwd() }),
		...(pid === undefined ? {} : { pid }),
		...(procStart === undefined ? {} : { procStart }),
	};
}

function refused(outcome: RouteOutcome, row?: RsRouteRow): RouteResult {
	return {
		outcome,
		...(row === undefined ? {} : { row }),
		rawEnvelope: routingRefusalEnvelope(outcome),
	};
}

function probeFailure(verb: string, probe: RsProbe): RouteOutcome | undefined {
	const { addr, stateDir } = probe.location;
	switch (probe.kind) {
		case "live":
			return undefined;
		case "absent":
			return unported(verb, addr, `no rs daemon answered at ${addr}`);
		case "no-credentials":
			return unported(verb, addr, `no rs credential at ${stateDir}/daemon.key`);
		case "auth-rejected":
			return { kind: "rs-auth-rejected", verb, addr, stateDir };
		case "unreadable":
			return { kind: "rs-unreadable", verb, addr, detail: probe.detail };
	}
}

/** No shell, buffering, parser, or HTTP substitute for the native trailer command. */
async function runNative(args: readonly string[], deps: GenerationRouterDeps): Promise<number> {
	if (deps.runNative !== undefined) return deps.runNative(args);
	return new Promise((resolve) => {
		const child = spawn("pij-rs", [...args], {
			env: deps.env,
			cwd: deps.cwd?.(),
			stdio: "inherit",
		});
		const forwardInt = () => {
			child.kill("SIGINT");
		};
		const forwardTerm = () => {
			child.kill("SIGTERM");
		};
		const forwardHup = () => {
			child.kill("SIGHUP");
		};
		const finish = (code: number) => {
			process.off("SIGINT", forwardInt);
			process.off("SIGTERM", forwardTerm);
			process.off("SIGHUP", forwardHup);
			resolve(code);
		};
		process.on("SIGINT", forwardInt);
		process.on("SIGTERM", forwardTerm);
		process.on("SIGHUP", forwardHup);
		child.once("error", (error) => {
			process.stderr.write(`pij-rs commit-trailers: ${error.message}\n`);
			finish(127);
		});
		child.once("close", (code, signal) => {
			const signalNumber = signal === null ? undefined : osConstants.signals[signal];
			finish(code ?? (signalNumber === undefined ? 1 : 128 + signalNumber));
		});
	});
}

/** Syntactic forwarding only: the existing daemon applies these filters. */
function readQuery(
	row: RsHttpRoute,
	argv: readonly string[],
	cwd: (() => string) | undefined,
):
	| { readonly ok: true; readonly query: URLSearchParams }
	| { readonly ok: false; readonly reason: string } {
	const query = new URLSearchParams();
	for (let index = 1; index < argv.length; index++) {
		const token = argv[index];
		if (token === undefined)
			return { ok: false, reason: `missing read argument at position ${index}` };
		if (token === "--json") continue;
		const matched = /^--([^=]+)(?:=(.*))?$/.exec(token);
		const name = matched?.[1];
		if (name === undefined || row.query === undefined || !Object.hasOwn(row.query, name)) {
			return { ok: false, reason: `unsupported read argument ${token}` };
		}
		const inline = matched?.[2];
		const value = inline ?? (name === "here" ? "true" : argv[++index]);
		if (value === undefined || value === "" || (inline === undefined && value.startsWith("--"))) {
			return { ok: false, reason: `--${name} requires a value` };
		}
		if (query.has(name)) return { ok: false, reason: `duplicate read argument --${name}` };
		const accepted = row.query[name];
		if (accepted !== null && accepted !== undefined && !accepted.includes(value)) {
			return { ok: false, reason: `unsupported --${name} value ${value}` };
		}
		query.set(name, value);
	}
	for (const [name, value] of Object.entries(row.fixedQuery ?? {})) query.set(name, value);
	if (query.has("here")) {
		if (query.get("here") === "false") query.delete("here");
		else {
			const folder = cwd?.();
			if (!folder) return { ok: false, reason: "--here requires caller cwd" };
			query.set("here", folder);
		}
	}
	return { ok: true, query };
}

export async function routeVerb(
	verb: string,
	leaf: string | undefined,
	argv: readonly string[],
	deps: GenerationRouterDeps = defaultRouterDeps(),
	table: readonly RsRouteRow[] = RS_ROUTE_TABLE,
): Promise<RouteResult> {
	const location = daemonLocation(deps.env, deps.home);
	const force = resolveGenerationForce(deps.env);
	// Decide status before touching a credential, a roster, or the network.
	const pre = decidePreCall({ verb, leaf, force, addr: location.addr, rsLive: true }, table);
	if (pre.kind !== "try-rs") return refused(pre, findRoute(verb, leaf, table));
	const row = pre.row;
	if ("nativeCommand" in row) {
		const exitCode = await runNative(
			[
				"--state-dir",
				location.stateDir,
				"--addr",
				location.addr,
				row.nativeCommand,
				...argv.slice(1),
			],
			deps,
		);
		return { row, outcome: { kind: "rs-native", verb, addr: location.addr, exitCode } };
	}
	let query: URLSearchParams | undefined;
	if (row.method === "GET") {
		const parsed = readQuery(row, argv, deps.cwd);
		if (!parsed.ok) return refused(unported(verb, location.addr, parsed.reason), row);
		query = parsed.query;
	}
	const probe = await probeRs(deps);
	const failure = probeFailure(verb, probe);
	if (failure !== undefined)
		return probe.rawEnvelope === undefined
			? refused(failure, row)
			: { outcome: failure, row, rawEnvelope: probe.rawEnvelope };
	let caller = buildCallerContext(deps.env, callerProcessFacts(deps));
	const register = verb === "inbox" && leaf === "register";
	if (register && argv.filter((arg) => arg !== "--json").join(" ") !== "inbox register")
		return refused(
			{ kind: "rs-error", verb, addr: location.addr, detail: "inbox register takes only --json" },
			row,
		);
	if (!caller.tmuxPane && ["inbox", "send", "whoami", "phonehome"].includes(verb)) {
		const identity = deps.ambientIdentity?.();
		if (identity) {
			const registerRow = findRoute("inbox", "register", table);
			if (registerRow === undefined || !("rsPath" in registerRow))
				return refused(
					unported(
						verb,
						location.addr,
						"native ambient registration is not in this routing inventory",
					),
					row,
				);
			if (caller.pid === undefined || caller.procStart === undefined || !caller.cwd)
				return refused(
					{
						kind: "rs-error",
						verb,
						addr: location.addr,
						detail: "ambient registration requires observed caller process start and cwd",
					},
					row,
				);
			const registration: Registration = {
				id: "",
				harness: identity.harness,
				harness_session: identity.harnessSessionId,
				folder: caller.cwd,
				pid: caller.pid,
				proc_start: caller.procStart,
				relay: false,
				...(caller.pijParentId === undefined ? {} : { parent: caller.pijParentId }),
			};
			const registered = await callRs(
				"inbox",
				registerRow,
				["inbox", "register"],
				location,
				deps,
				force === "rs",
				caller,
				undefined,
				registration,
			);
			if (registered.outcome.kind !== "rs") return registered;
			const descriptor = registered.payload as RustSeatDescriptor | undefined;
			const host = descriptor?.proc;
			if (
				!descriptor?.id ||
				descriptor.harness !== identity.harness ||
				descriptor.pane != null ||
				!host ||
				!Number.isSafeInteger(host.pid) ||
				host.pid <= 0 ||
				!Number.isSafeInteger(host.proc_start) ||
				host.proc_start <= 0
			)
				return refused(
					{
						kind: "rs-unreadable",
						verb,
						addr: location.addr,
						detail: "native registration returned no matching paneless host identity",
					},
					row,
				);
			if (register) return registered;
			// The daemon observed this host; an inherited PIJ_SESSION_ID never overrides it.
			caller = {
				...caller,
				pijSessionId: descriptor.id,
				pid: host.pid,
				procStart: host.proc_start,
			};
			if (identity.harness === "claude")
				caller = { ...caller, claudeCodeSessionId: identity.harnessSessionId };
			else if (identity.harness === "copilot")
				caller = { ...caller, copilotAgentSessionId: identity.harnessSessionId };
			else caller = { ...caller, codexThreadId: identity.harnessSessionId };
		}
	}
	if (register) {
		if (!caller.tmuxPane && !caller.pijSessionId)
			return refused(
				{
					kind: "rs-error",
					verb,
					addr: location.addr,
					code: "E-AMBIG",
					detail:
						"cannot detect a current Claude, Copilot, or Codex session; run inside an agent tool shell",
				},
				row,
			);
		// Already-admitted panes/IDs are reads, not a second registration or a client minter.
		return callRs(
			verb,
			{ ...row, rsPath: "/v1/whoami" },
			["whoami"],
			location,
			deps,
			force === "rs",
			caller,
		);
	}
	return callRs(verb, row, argv, location, deps, force === "rs", caller, query);
}

async function callRs(
	verb: string,
	row: RsHttpRoute,
	argv: readonly string[],
	location: DaemonLocation,
	deps: GenerationRouterDeps,
	forced: boolean,
	caller: CallerContext,
	query?: URLSearchParams,
	registration?: Registration,
): Promise<RouteResult> {
	const { addr, stateDir } = location;
	let key: string;
	try {
		key = (await deps.readFile(join(stateDir, "daemon.key"), "utf8")).trim();
	} catch {
		return refused(unported(verb, addr, `no rs credential at ${stateDir}/daemon.key`), row);
	}
	if (key === "")
		return refused(unported(verb, addr, `empty rs credential at ${stateDir}/daemon.key`), row);
	const suffix = query?.toString();
	const url = `http://${addr}${row.rsPath}${suffix ? `?${suffix}` : ""}`;
	const headers = { Authorization: `Bearer ${key}` };
	let bodyLiteral: string | undefined;
	const bodyFileAt = row.readsBodyFile ? argv.indexOf("--body-file") : -1;
	if (bodyFileAt !== -1) {
		const path = argv[bodyFileAt + 1];
		if (path === undefined || path.startsWith("--"))
			return refused(
				{ kind: "rs-error", verb, addr, detail: "--body-file takes a path (or - for stdin)" },
				row,
			);
		try {
			bodyLiteral = await deps.readBody(path);
		} catch (error) {
			return refused(
				{
					kind: "rs-error",
					verb,
					addr,
					detail: `--body-file: ${error instanceof Error ? error.message : String(error)}`,
				},
				row,
			);
		}
	}
	let response: Response;
	let body: string;
	try {
		response = await deps.fetch(
			url,
			row.method === "GET"
				? { headers }
				: {
						method: "POST",
						headers: { ...headers, "Content-Type": "application/json" },
						body: JSON.stringify(
							registration ??
								(bodyLiteral === undefined
									? { argv, caller }
									: { argv, caller, body_literal: bodyLiteral }),
						),
					},
		);
		body = await response.text();
	} catch (error) {
		return refused(
			{
				kind: "rs-error",
				verb,
				addr,
				detail: error instanceof Error ? error.message : String(error),
			},
			row,
		);
	}
	let envelopeDecoded = false;
	let wasRefused = false;
	let payload: unknown;
	let detail: string | undefined;
	try {
		payload = decodeEnvelope<unknown>(body).data;
		envelopeDecoded = true;
	} catch (error) {
		if (error instanceof DaemonRefusalError) {
			envelopeDecoded = true;
			wasRefused = true;
		}
		detail = error instanceof Error ? error.message : String(error);
	}
	const outcome = classifyRsResponse(
		verb,
		addr,
		row,
		{ status: response.status, envelopeDecoded, refused: wasRefused, detail },
		forced,
		stateDir,
	);
	if (!envelopeDecoded) return refused(outcome, row);
	if (outcome.kind !== "rs") return { outcome, row, rawEnvelope: body };
	const acknowledge =
		row.acknowledgePath === undefined
			? undefined
			: buildAcknowledge(row.acknowledgePath, payload, url, headers, deps, caller);
	return {
		outcome,
		payload,
		row,
		rawEnvelope: body,
		...(acknowledge === undefined ? {} : { acknowledge }),
	};
}

function buildAcknowledge(
	path: string,
	payload: unknown,
	url: string,
	headers: Record<string, string>,
	deps: GenerationRouterDeps,
	caller: CallerContext,
): (() => Promise<string | undefined>) | undefined {
	const claims: unknown[] = Array.isArray(payload) ? payload : [];
	const jobIds = claims
		.map((claim) => (claim as { readonly job_id?: unknown } | null)?.job_id)
		.filter(
			(jobId): jobId is string | number => typeof jobId === "string" || typeof jobId === "number",
		);
	if (jobIds.length === 0) return undefined;
	const ackUrl = new URL(path, url).href;
	return async () => {
		for (const jobId of jobIds) {
			try {
				const response = await deps.fetch(ackUrl, {
					method: "POST",
					headers: { ...headers, "Content-Type": "application/json" },
					body: JSON.stringify({ caller, job_id: jobId }),
				});
				if (!response.ok)
					return `messages were received but acknowledgement failed; they may be read again (HTTP ${response.status})`;
				decodeEnvelope<unknown>(await response.text());
			} catch (error) {
				return `messages were received but acknowledgement failed; they may be read again: ${error instanceof Error ? error.message : String(error)}`;
			}
		}
		return undefined;
	};
}

export interface GenerationDecision {
	readonly verb: string;
	readonly generation: "rs" | "rs-failed";
	readonly addr: string;
	readonly line: string;
}

/** Read-only capability diagnostic; never invokes a row or reads legacy stores. */
export async function probeDecisionOnly(
	verbs?: readonly string[],
	deps: GenerationRouterDeps = defaultRouterDeps(),
): Promise<readonly GenerationDecision[]> {
	const force = resolveGenerationForce(deps.env);
	const location = daemonLocation(deps.env, deps.home);
	const asked =
		verbs === undefined
			? RS_ROUTE_TABLE.map((row) => ({ verb: row.verb, leaf: row.leaf }))
			: verbs.flatMap((verb) => {
					const rows = RS_ROUTE_TABLE.filter((row) => row.verb === verb);
					return rows.length === 0
						? [{ verb, leaf: undefined }]
						: rows.map((row) => ({ verb, leaf: row.leaf }));
				});
	const needsProbe =
		force !== "legacy" &&
		asked.some(({ verb, leaf }) => {
			const row = findRoute(verb, leaf);
			return row !== undefined && "rsPath" in row;
		});
	const probe = needsProbe ? await probeRs(deps) : undefined;
	return asked.map(({ verb, leaf }) => {
		const label = leaf === undefined ? verb : `${verb} ${leaf}`;
		const pre = decidePreCall({ verb, leaf, force, addr: location.addr, rsLive: true });
		let failure: RouteOutcome | undefined;
		if (pre.kind !== "try-rs") failure = pre;
		else if ("rsPath" in pre.row && probe !== undefined) failure = probeFailure(verb, probe);
		if (failure !== undefined)
			return {
				verb: label,
				generation: "rs-failed",
				addr: location.addr,
				line: describeOutcome({ ...failure, verb: label }),
			};
		const row = findRoute(verb, leaf);
		const destination =
			row !== undefined && "nativeCommand" in row
				? `pij-rs ${row.nativeCommand}`
				: row !== undefined && "rsPath" in row
					? row.rsPath
					: label;
		return {
			verb: label,
			generation: "rs",
			addr: location.addr,
			line: `${label}: would be attempted against rs (${destination}); no operation was performed`,
		};
	});
}
