import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { decidePreCall, findRoute, renderRsAnswer } from "../core/generation-routing.js";
import { type GenerationRouterDeps, routeVerb } from "./generation-router.js";

// HTTP-boundary proof only: every fetch, key read and caller lookup is injected.
// No daemon, socket, registry, native CLI, tmux or model runs here. In particular,
// these tests do not prove Rust filter semantics, control delivery or job execution.
interface Envelope {
	readonly ok: boolean;
	readonly command: string;
	readonly v: number;
	readonly data?: unknown;
	readonly meta?: string;
	readonly details?: { readonly code: string };
}
interface FixtureRequest {
	readonly argv?: readonly string[];
	readonly caller?: { readonly TMUX_PANE?: string; readonly cwd?: string };
}
interface FixtureCase {
	readonly id: string;
	readonly request: FixtureRequest | null;
	readonly shim_request?: FixtureRequest;
	readonly response: Envelope;
}
interface RouteFixture {
	readonly method: string;
	readonly path: string;
	readonly cases: readonly FixtureCase[];
}
const contracts = JSON.parse(
	readFileSync(
		new URL(
			"../../../../crates/testkit/fixtures/golden/api/governance-routes.json",
			import.meta.url,
		),
		"utf8",
	),
) as {
	readonly routes: readonly RouteFixture[];
	readonly fixture_context: { readonly parent: string; readonly worker: string };
	readonly refusals: Record<string, { readonly http_status: number; readonly response: Envelope }>;
};
const eventContracts = JSON.parse(
	readFileSync(
		new URL(
			"../../../../crates/testkit/fixtures/golden/api/governance-events.json",
			import.meta.url,
		),
		"utf8",
	),
) as { readonly events: readonly { readonly id: string; readonly decoded_payload: unknown }[] };

const postCases = contracts.routes.flatMap((route) =>
	route.method !== "POST"
		? []
		: route.cases.flatMap((fixture) => {
				const request = fixture.shim_request ?? fixture.request;
				const argv = request?.argv;
				return argv === undefined
					? []
					: [{ id: fixture.id, path: route.path, request, argv, response: fixture.response }];
			}),
);

const KEY = "c".repeat(64);
const ADDR = "127.0.0.1:17467";
const HEALTH = JSON.stringify({
	ok: true,
	command: "pij ping",
	v: 2,
	data: { status: "healthy", build: "u6-fake-host", offline: false, machine: "u6-fake-host" },
});
const AUTH = JSON.stringify({
	ok: false,
	command: "auth",
	v: 2,
	error: "auth",
	meta: "missing key",
});

interface RequestSeen {
	readonly path: string;
	readonly method: string;
	readonly body: string | undefined;
	readonly authorization: string | null;
}
function fakeHost(
	answer: (path: string, init?: RequestInit) => Response,
	over: Partial<GenerationRouterDeps> = {},
) {
	const requests: RequestSeen[] = [];
	const deps: GenerationRouterDeps = {
		fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
			const url = new URL(String(input));
			requests.push({
				path: `${url.pathname}${url.search}`,
				method: init?.method ?? "GET",
				body: typeof init?.body === "string" ? init.body : undefined,
				authorization: new Headers(init?.headers).get("Authorization"),
			});
			return answer(url.pathname, init);
		}) as typeof fetch,
		readFile: (async () => KEY) as GenerationRouterDeps["readFile"],
		readBody: async () => {
			throw new Error("unexpected body-file read");
		},
		env: { PIJ_RS_ADDR: ADDR, PIJ_RS_STATE_DIR: "/unused-u6-state" },
		home: "/unused-u6-home",
		...over,
	};
	return { deps, requests };
}
function replying(raw: string, status = 200, over: Partial<GenerationRouterDeps> = {}) {
	return fakeHost(
		(path) =>
			new Response(path === "/health" ? HEALTH : raw, {
				status: path === "/health" ? 200 : status,
			}),
		over,
	);
}
function leafOf(argv: readonly string[]): string | undefined {
	const leaf = argv[1];
	return leaf?.startsWith("--") ? undefined : leaf;
}
function fixtureNamed(id: string) {
	const fixture = postCases.find((entry) => entry.id === id);
	if (fixture === undefined) throw new Error(`Missing canonical argv fixture: ${id}`);
	return fixture;
}
function canonicalBytes(response: Envelope): string {
	// Deliberate whitespace makes decode/re-stringify fail byte equality. The
	// envelope itself comes solely from the stable contract, not a second schema.
	return `${JSON.stringify(response, null, 2)}\n`;
}
function callerDeps(request: FixtureRequest | null | undefined): Partial<GenerationRouterDeps> {
	return {
		env: {
			PIJ_RS_ADDR: ADDR,
			PIJ_RS_STATE_DIR: "/unused-u6-state",
			TMUX_PANE: request?.caller?.TMUX_PANE,
			SECRET_TOKEN: "must-not-leave-the-shim",
		},
		cwd: () => request?.caller?.cwd ?? "/unused-u6-cwd",
	};
}

describe("rs cutover has no legacy escape", () => {
	it.each([
		undefined,
		"legacy",
		"rs",
	] as const)("an unknown verb refuses by name with generation force %s", (force) => {
		const outcome = decidePreCall({
			verb: "synthetic-unported-verb",
			force,
			addr: ADDR,
			rsLive: true,
		});
		expect(outcome.kind.startsWith("legacy-")).toBe(false);
		expect(outcome.kind).not.toBe("try-rs");
		expect(outcome).toMatchObject({ detail: expect.stringContaining("E-RS-UNPORTED") });
	});

	it("PIJ_DAEMON_GENERATION=legacy cannot dispatch even a formerly routed verb to legacy", async () => {
		const host = replying(HEALTH, 200, { env: { PIJ_DAEMON_GENERATION: "legacy" } });
		const result = await routeVerb("whoami", undefined, ["whoami"], host.deps);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
		expect(result.outcome).toMatchObject({ detail: expect.stringContaining("E-RS-UNPORTED") });
	});

	it("an absent daemon is a refusal, never a legacy fallback", async () => {
		const host = fakeHost(() => {
			throw Object.assign(new Error("fetch failed"), { cause: { code: "ECONNREFUSED" } });
		});
		const result = await routeVerb("whoami", undefined, ["whoami"], host.deps);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
	});

	it("a missing bearer key cannot rehome the caller into legacy", async () => {
		const host = fakeHost(() => new Response(AUTH, { status: 401 }), {
			readFile: (async () => {
				throw new Error("ENOENT");
			}) as GenerationRouterDeps["readFile"],
		});
		const result = await routeVerb("whoami", undefined, ["whoami"], host.deps);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
	});

	it("a bodiless HTTP 404 from a live daemon cannot fall through to legacy", async () => {
		const host = replying("", 404);
		const result = await routeVerb("whoami", undefined, ["whoami"], host.deps);
		expect(host.requests.some((request) => request.path === "/v1/whoami")).toBe(true);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
	});

	it.each([
		"inbox",
		"report",
		"project",
	])("an unlisted %s leaf never falls through to legacy", async (verb) => {
		const refusal = contracts.refusals.unported;
		if (refusal === undefined) throw new Error("Missing canonical unported refusal fixture");
		const host = replying(canonicalBytes(refusal.response), refusal.http_status);
		const result = await routeVerb(verb, "synthetic-leaf", [verb, "synthetic-leaf"], host.deps);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
		expect(result.outcome).toMatchObject({ detail: expect.stringContaining("E-RS-UNPORTED") });
	});

	it("a legacy-only seat does not select legacy for an rs request", async () => {
		const host = replying(HEALTH);
		const deps = {
			...host.deps,
			ambientSeatId: () => "legacy-only-seat",
			seatInLegacy: () => true,
			seatInRs: () => false,
		};
		const result = await routeVerb("whoami", undefined, ["whoami"], deps);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
	});

	it.each([
		["list", ["--role", "reviewer"]],
		["sessions", ["--here"]],
		["sessions", ["--parent", "pij-parent"]],
	] as const)("%s %j refuses unsupported GET filters instead of ignoring them", async (verb, filters) => {
		// These flags are outside the declared shim GET contract. The seats API
		// has other filters; silently dropping these would still be a false answer.
		const host = replying(HEALTH);
		const result = await routeVerb(verb, undefined, [verb, ...filters, "--json"], {
			...host.deps,
			cwd: () => "/work/requested-scope",
		});
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
		expect(result.outcome).toMatchObject({ detail: expect.stringContaining("E-RS-UNPORTED") });
		expect(host.requests.filter((request) => request.path !== "/health")).toEqual([]);
	});
});

describe("governance shim preserves canonical HTTP contracts", () => {
	it.each(
		postCases,
	)("$id POST preserves argv, caller and complete v2 success bytes", async (fixture) => {
		const argv = [...fixture.argv, "--json"];
		const verb = fixture.argv[0];
		if (verb === undefined) throw new Error(`Missing canonical verb: ${fixture.id}`);
		const leaf = leafOf(fixture.argv);
		const raw = canonicalBytes(fixture.response);
		const host = replying(raw, 200, callerDeps(fixture.request));
		const result = await routeVerb(verb, leaf, argv, host.deps);
		expect(result.outcome.kind).toBe("rs");
		expect(findRoute(verb, leaf)).toBeDefined();
		expect(host.requests.filter((request) => request.path !== "/health")).toEqual([
			{
				path: fixture.path,
				method: "POST",
				authorization: `Bearer ${KEY}`,
				body: expect.any(String),
			},
		]);
		const body = host.requests.at(-1)?.body;
		if (body === undefined) throw new Error(`Missing POST request body: ${fixture.id}`);
		expect(JSON.parse(body)).toEqual({
			argv,
			caller: { tmuxPane: fixture.request?.caller?.TMUX_PANE, cwd: fixture.request?.caller?.cwd },
		});
		expect(result.payload).toEqual(fixture.response.data);
		expect(result.rawEnvelope).toBe(raw);
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});

	it.each(
		Object.entries(contracts.refusals).filter(([id]) => id !== "unported"),
	)("%s preserves complete canonical v2 refusal bytes rather than unwrapping or rewrapping", async (_id, refusal) => {
		const fixture = postCases.find((entry) => entry.response.command === refusal.response.command);
		if (fixture === undefined)
			throw new Error(`Missing canonical command fixture: ${refusal.response.command}`);
		const argv = [...fixture.argv, "--json"];
		const verb = fixture.argv[0];
		if (verb === undefined) throw new Error(`Missing canonical verb: ${fixture.id}`);
		const raw = canonicalBytes(refusal.response);
		const host = replying(raw, refusal.http_status, callerDeps(fixture.request));
		const result = await routeVerb(verb, leafOf(fixture.argv), argv, host.deps);
		expect(result.outcome.kind.startsWith("legacy-")).toBe(false);
		expect(result.outcome.kind).not.toBe("rs");
		expect(host.requests.at(-1)?.path).toBe(fixture.path);
		expect(result.rawEnvelope).toBe(raw);
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});

	it.each([
		[
			"anomalies-argv",
			["--here", "--project", "governance-port", "--seat", contracts.fixture_context.worker],
		],
		[
			"decisions-argv",
			[
				"--state",
				"all",
				"--asked_by",
				contracts.fixture_context.worker,
				"--parent",
				contracts.fixture_context.parent,
			],
		],
	] as const)("%s retains every filter and caller scope for the shared daemon parser", async (id, filters) => {
		const fixture = fixtureNamed(id);
		const argv = [...fixture.argv, ...filters, "--json"];
		const verb = fixture.argv[0];
		if (verb === undefined) throw new Error(`Missing canonical verb: ${fixture.id}`);
		const raw = canonicalBytes(fixture.response);
		const host = replying(raw, 200, callerDeps(fixture.request));
		const result = await routeVerb(verb, undefined, argv, host.deps);
		expect(result.outcome.kind).toBe("rs");
		expect(host.requests.at(-1)?.method).toBe("POST");
		expect(host.requests.at(-1)?.path).toBe(fixture.path);
		const body = host.requests.at(-1)?.body;
		if (body === undefined) throw new Error(`Missing POST request body: ${fixture.id}`);
		expect(JSON.parse(body)).toEqual({
			argv,
			caller: { tmuxPane: fixture.request?.caller?.TMUX_PANE, cwd: fixture.request?.caller?.cwd },
		});
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});

	it("keeps spine event payloads JSON strings matching the stable event contract", async () => {
		const fixture = fixtureNamed("spine-append");
		const event = eventContracts.events.find((entry) => entry.id === fixture.id);
		if (event === undefined) throw new Error(`Missing canonical event fixture: ${fixture.id}`);
		const raw = canonicalBytes(fixture.response);
		const argv = [...fixture.argv, "--json"];
		const host = replying(raw, 200, callerDeps(fixture.request));
		const result = await routeVerb("spine", "append", argv, host.deps);
		const rendered = renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope);
		expect(rendered).toEqual({ kind: "json", text: raw });
		if (rendered.kind !== "json") throw new Error("spine JSON response was refused");
		const payload = JSON.parse(rendered.text).data.event.payload;
		expect(typeof payload).toBe("string");
		expect(JSON.parse(payload)).toEqual(event.decoded_payload);
	});
});

describe("cutover retains shipped rs surfaces without legacy reads", () => {
	it("sessions neither reads legacy rows nor null-fills or unions their fields", async () => {
		const rsRow = {
			pijId: "pij-shared",
			harness: "omp",
			harnessSessionId: null,
			gitCommonDir: "/repo/.git",
			lifecycle: null,
			boundModel: null,
			spawnedBy: null,
			transcriptPath: null,
			generation: "rs",
		};
		const data = { rows: [rsRow] };
		const raw = canonicalBytes({ ok: true, command: "pij sessions", v: 2, data });
		let legacyReads = 0;
		const host = replying(raw);
		// An inferred object keeps this sentinel baseline-compatible even when the
		// cutover removes legacySessionRows from GenerationRouterDeps entirely.
		const deps = {
			...host.deps,
			legacySessionRows: () => {
				legacyReads++;
				return ["pij-shared", "pij-legacy-only"].map((pijId) => ({
					pijId,
					harness: "pi" as const,
					harnessSessionId: "legacy-session",
					lifecycle: "bound" as const,
					boundModel: "legacy-model",
					spawnedBy: "legacy-parent",
					transcriptPath: "/legacy/transcript",
					prime: false,
					oldPrime: false,
				}));
			},
		};
		const argv = ["sessions", "--json"];
		const result = await routeVerb("sessions", undefined, argv, deps);
		expect(result.outcome.kind).toBe("rs");
		expect(legacyReads).toBe(0);
		expect(result.payload).toEqual(data);
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});

	it.each([
		// Plan 160: the shim always asks for each seat's size.
		["list", "/v1/seats?sizes=true"],
		["sessions", "/v1/shim/sessions"],
	] as const)("bare %s keeps its real GET endpoint and complete envelope", async (verb, path) => {
		const raw = canonicalBytes({ ok: true, command: `pij ${verb}`, v: 2, data: { rows: [] } });
		const argv = [verb, "--json"];
		const host = replying(raw);
		const result = await routeVerb(verb, undefined, argv, host.deps);
		expect(result.outcome.kind).toBe("rs");
		expect(host.requests.at(-1)).toEqual({
			path,
			method: "GET",
			body: undefined,
			authorization: `Bearer ${KEY}`,
		});
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});

	it("list preserves and URL-encodes every declared GET filter without rewriting the response", async () => {
		const filters = {
			harness: "omp",
			folder: "/work/team & contracts/#draft?50%+résumé",
			parent: contracts.fixture_context.parent,
			scope: "local",
		};
		const argv = [
			"list",
			...Object.entries(filters).flatMap(([key, value]) => [`--${key}`, value]),
			"--json",
		];
		const raw = canonicalBytes({ ok: true, command: "pij list", v: 2, data: [] });
		const host = replying(raw);
		const result = await routeVerb("list", undefined, argv, host.deps);
		expect(result.outcome.kind).toBe("rs");
		const requests = host.requests.filter((request) => request.path !== "/health");
		expect(requests).toHaveLength(1);
		expect(requests[0]).toMatchObject({
			method: "GET",
			body: undefined,
			authorization: `Bearer ${KEY}`,
		});
		const request = requests[0];
		if (request === undefined) throw new Error("Missing list GET request");
		const url = new URL(request.path, `http://${ADDR}`);
		expect(url.pathname).toBe("/v1/seats");
		expect(url.hash).toBe("");
		expect([...url.searchParams]).toHaveLength(Object.keys(filters).length + 1);
		expect(Object.fromEntries(url.searchParams)).toEqual({ ...filters, sizes: "true" });
		expect(result.rawEnvelope).toBe(raw);
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});

	it.each([
		0, 7,
	])("commit-trailers uses only native forwarding and preserves exit status %s", async (exitCode) => {
		const argv = ["commit-trailers", "--json"];
		const calls: string[][] = [];
		const host = replying(HEALTH);
		// Keep the injected extension on an inferred object, so the original
		// adapter can execute this test and fail behavior rather than type imports.
		const deps = {
			...host.deps,
			runNative: async (args: readonly string[]) => {
				calls.push([...args]);
				return exitCode;
			},
		};
		const result = await routeVerb("commit-trailers", undefined, argv, deps);
		expect(calls).toEqual([["--state-dir", "/unused-u6-state", "--addr", ADDR, ...argv]]);
		expect(result.outcome).toMatchObject({ kind: "rs-native", exitCode });
		expect(host.requests).toEqual([]);
	});

	it("an unported verb cannot use native forwarding as another escape hatch", async () => {
		let nativeCalls = 0;
		const host = replying(HEALTH);
		const deps = {
			...host.deps,
			runNative: async () => {
				nativeCalls++;
				return 0;
			},
		};
		const result = await routeVerb(
			"synthetic-unported-verb",
			undefined,
			["synthetic-unported-verb"],
			deps,
		);
		expect(nativeCalls).toBe(0);
		expect(result.outcome).toMatchObject({ detail: expect.stringContaining("E-RS-UNPORTED") });
	});

	it.each([
		["send", ["send", "pij-target", "--command", "compact"], "/v1/shim/send"],
		["send", ["send", "pij-target", "--command", "new"], "/v1/shim/send"],
		["send", ["send", "pij-target", "--command", "reload"], "/v1/shim/send"],
		["compact-self", ["compact-self"], "/v1/shim/compact-self"],
	] as const)("%s %j still reaches the shipped control endpoint with caller evidence", async (verb, argv, path) => {
		const host = replying(
			JSON.stringify({
				ok: true,
				command: `pij ${verb}`,
				v: 2,
				data: { msg_id: "control-fixture", outcome: "accepted", at: 1 },
			}),
			200,
			{
				env: {
					PIJ_RS_ADDR: ADDR,
					PIJ_RS_STATE_DIR: "/unused-u6-state",
					TMUX_PANE: "%control",
					PIJ_SESSION_ID: "pij-control",
				},
			},
		);
		const deps = {
			...host.deps,
			ambientSeatId: () => "pij-control",
			seatInRs: () => true,
		};
		const result = await routeVerb(verb, undefined, argv, deps);
		expect(result.outcome.kind).toBe("rs");
		expect(host.requests.at(-1)?.path).toBe(path);
		expect(host.requests.at(-1)?.method).toBe("POST");
		const body = host.requests.at(-1)?.body;
		if (body === undefined) throw new Error(`Missing control POST request body: ${verb}`);
		expect(JSON.parse(body)).toEqual({
			argv,
			caller: { tmuxPane: "%control", pijSessionId: "pij-control" },
		});
	});

	it.each([
		["create", ["--title", "cutover", "--command", "printf '%s\\n' '--json'"]],
		["list", ["--all"]],
		["tail", ["bg-golden", "--lines", "2"]],
		["kill", ["bg-golden"]],
	] as const)("bg %s retains its shipped endpoint and native golden envelope", async (leaf, args) => {
		const raw = readFileSync(
			new URL(
				`../../../../crates/testkit/fixtures/golden/cli/bg-${leaf}-envelope.json`,
				import.meta.url,
			),
			"utf8",
		);
		const argv = ["bg", leaf, ...args, "--json"];
		const host = replying(raw);
		const result = await routeVerb("bg", leaf, argv, host.deps);
		expect(result.outcome.kind).toBe("rs");
		expect(host.requests.at(-1)?.path).toBe("/v1/bg");
		expect(host.requests.at(-1)?.method).toBe("POST");
		const body = host.requests.at(-1)?.body;
		if (body === undefined) throw new Error(`Missing bg POST request body: ${leaf}`);
		expect(JSON.parse(body)).toEqual({ argv, caller: {} });
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: raw,
		});
	});
});
