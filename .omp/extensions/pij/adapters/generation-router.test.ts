import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { COPILOT_CONTROL_REFUSAL } from "../core/commands.js";
import { describeOutcome, renderRsAnswer } from "../core/generation-routing.js";
import { type GenerationRouterDeps, probeRs, routeVerb } from "./generation-router.js";

const KEY = "k".repeat(64);
const ADDR = "127.0.0.1:18743";
const STATE_DIR = "/tmp/pij-router-fixture";
const HEALTH = JSON.stringify({
	ok: true,
	command: "pij ping",
	v: 2,
	data: { status: "healthy", build: "pij-rs test", offline: false, machine: "test" },
});
const AUTH_401 = JSON.stringify({
	ok: false,
	command: "auth",
	v: 2,
	meta: "missing or wrong bearer token",
	error: "auth",
});
const connectionRefused = () => {
	const error = new Error("fetch failed");
	(error as { cause?: unknown }).cause = { code: "ECONNREFUSED" };
	return error;
};

interface Call {
	readonly url: string;
	readonly method: string;
}

function deps(
	handler: (url: string, init?: RequestInit) => Response | Promise<Response>,
	over: Partial<GenerationRouterDeps> = {},
	calls: Call[] = [],
): GenerationRouterDeps {
	return {
		fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
			const url = String(input);
			calls.push({ url, method: init?.method ?? "GET" });
			return handler(url, init);
		}) as unknown as typeof fetch,
		readFile: (async (path: unknown) => {
			if (String(path) !== `${STATE_DIR}/daemon.key`)
				throw new Error(`unexpected file read: ${String(path)}`);
			return KEY;
		}) as unknown as GenerationRouterDeps["readFile"],
		readBody: async (path) => {
			throw new Error(`unexpected body read: ${path}`);
		},
		home: "/tmp/pij-router-home-fixture",
		...over,
		env: { PIJ_RS_ADDR: ADDR, PIJ_RS_STATE_DIR: STATE_DIR, ...over.env },
	};
}

function answering(rawEnvelope: string, status = 200) {
	return (url: string) =>
		new Response(url.endsWith("/health") ? HEALTH : rawEnvelope, {
			status: url.endsWith("/health") ? 200 : status,
		});
}

const keyless = () =>
	deps(() => new Response(AUTH_401, { status: 401 }), {
		readFile: (async () => {
			throw new Error("ENOENT");
		}) as unknown as GenerationRouterDeps["readFile"],
	});

describe("probing rs through the shipped detector", () => {
	it("a good key and listening daemon are live at the configured address", async () => {
		const probe = await probeRs(deps(() => new Response(HEALTH)));
		expect(probe).toMatchObject({ kind: "live", location: { addr: ADDR, stateDir: STATE_DIR } });
	});

	it("connection refusal is absent, not authentication rejection", async () => {
		expect(
			(
				await probeRs(
					deps(() => {
						throw connectionRefused();
					}),
				)
			).kind,
		).toBe("absent");
	});

	it("a present but rejected key is not absent", async () => {
		expect((await probeRs(deps(() => new Response(AUTH_401, { status: 401 })))).kind).toBe(
			"auth-rejected",
		);
	});

	it("an absent or empty key is no-credentials, not a rejected stored key", async () => {
		expect((await probeRs(keyless())).kind).toBe("no-credentials");
		expect(
			(
				await probeRs(
					deps(() => new Response(AUTH_401, { status: 401 }), {
						readFile: (async () => "") as unknown as GenerationRouterDeps["readFile"],
					}),
				)
			).kind,
		).toBe("no-credentials");
	});

	it.each([
		"",
		"<html>proxy error</html>",
		"{",
		"null",
		'{"ok":true,"command":"pij ping","v":99,"data":{}}',
	])("invalid or skewed health is a named unreadable result: %s", async (body) => {
		expect((await probeRs(deps(() => new Response(body)))).kind).toBe("unreadable");
	});
});

describe("routeVerb end to end", () => {
	it.each([
		["create", ["--title", "shell output", "--command", "printf '%s\\n' '--json'; echo done"]],
		["list", ["--all"]],
		["tail", ["bg-golden", "--lines", "2"]],
		["kill", ["bg-golden"]],
	] as const)("bg %s forwards caller and the complete native envelope golden", async (leaf, args) => {
		const rawEnvelope = readFileSync(
			new URL(
				`../../../../crates/testkit/fixtures/golden/cli/bg-${leaf}-envelope.json`,
				import.meta.url,
			),
			"utf8",
		);
		const base = ["bg", leaf, ...args];
		for (const jsonAt of [0, 1, 2, base.length, undefined]) {
			const argv = [...base];
			if (jsonAt !== undefined) argv.splice(jsonAt, 0, "--json");
			let request: unknown;
			const result = await routeVerb(
				"bg",
				leaf,
				argv,
				deps(
					(url, init) => {
						if (url.endsWith("/health")) return new Response(HEALTH);
						expect(new URL(url).pathname).toBe("/v1/bg");
						expect(init?.method).toBe("POST");
						expect(init?.headers).toMatchObject({ Authorization: `Bearer ${KEY}` });
						request = JSON.parse(String(init?.body));
						return new Response(rawEnvelope);
					},
					{
						env: {
							PIJ_SESSION_ID: "pij-bg-owner",
							TMUX_PANE: "%bg-cli",
							PIJ_PARENT_ID: "pij-bg-parent",
							HARNESS_SESSION_ID: "not-allowlisted",
						},
						cwd: () => "/work/bg-fixture",
					},
				),
			);
			expect(result.outcome.kind).toBe("rs");
			expect(request).toEqual({
				argv,
				caller: {
					pijSessionId: "pij-bg-owner",
					tmuxPane: "%bg-cli",
					pijParentId: "pij-bg-parent",
					cwd: "/work/bg-fixture",
				},
			});
			expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual(
				jsonAt === undefined
					? { kind: "rendered", text: JSON.parse(rawEnvelope).data.line }
					: { kind: "json", text: rawEnvelope },
			);
		}
	});

	it("bg does not mistake a title value for send's body-file transport", async () => {
		const argv = ["bg", "create", "--title", "--body-file", "--command", "echo done"];
		let request: unknown;
		const result = await routeVerb(
			"bg",
			"create",
			argv,
			deps((url, init) => {
				if (url.endsWith("/health")) return new Response(HEALTH);
				request = JSON.parse(String(init?.body));
				return new Response(
					JSON.stringify({ ok: true, command: "pij bg create", v: 2, data: { line: "started" } }),
				);
			}),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(request).toEqual({ argv, caller: {} });
	});

	it.each([
		200, 400, 403, 404, 500,
	])("preserves a decoded HTTP %s refusal byte-for-byte", async (status) => {
		const rawEnvelope =
			' {"ok":false,"command":"pij bg kill","v":2,"meta":"not the owner","error":"refused","details":{"code":"E-OWNER","extra":[1,2]}}\n';
		const argv = ["bg", "kill", "bg-other", "--json"];
		const result = await routeVerb("bg", "kill", argv, deps(answering(rawEnvelope, status)));
		expect(result.outcome).toMatchObject({ kind: "rs-error", detail: "not the owner" });
		expect(result.payload).toBeUndefined();
		expect(renderRsAnswer(result.row, result.payload, argv, result.rawEnvelope)).toEqual({
			kind: "json",
			text: rawEnvelope,
		});
	});

	it.each([
		["send", ["send", "pij-target", "--command", "compact"], "/v1/shim/send"],
		["compact-self", ["compact-self"], "/v1/shim/compact-self"],
	] as const)("%s forwards control argv and caller proof intact", async (verb, argv, path) => {
		let request: unknown;
		const result = await routeVerb(
			verb,
			undefined,
			argv,
			deps(
				(url, init) => {
					if (url.endsWith("/health")) return new Response(HEALTH);
					expect(new URL(url).pathname).toBe(path);
					request = JSON.parse(String(init?.body));
					return new Response(
						JSON.stringify({
							ok: true,
							command: `pij ${verb}`,
							v: 2,
							data: { msg_id: "control-1", outcome: "accepted", at: 1 },
						}),
					);
				},
				{ env: { TMUX_PANE: "%42", PIJ_SESSION_ID: "pij-self" } },
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(request).toEqual({ argv, caller: { tmuxPane: "%42", pijSessionId: "pij-self" } });
	});

	it("a daemon control refusal is not replaced with a client-side success", async () => {
		const rawEnvelope = JSON.stringify({
			ok: false,
			command: "pij compact-self",
			v: 2,
			error: "refused",
			meta: COPILOT_CONTROL_REFUSAL,
		});
		const result = await routeVerb(
			"compact-self",
			undefined,
			["compact-self"],
			deps(answering(rawEnvelope, 400)),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", detail: COPILOT_CONTROL_REFUSAL });
		expect(result.rawEnvelope).toBe(rawEnvelope);
	});

	it("a genuine not-found envelope is a daemon refusal, not an absent route", async () => {
		const corpus = JSON.parse(
			readFileSync(
				new URL(
					"../../../../crates/testkit/fixtures/golden/api/error-envelopes.json",
					import.meta.url,
				),
				"utf8",
			),
		) as { error: string; meta: string }[];
		const fixture = corpus.find((envelope) => envelope.error === "not_found");
		if (fixture === undefined) throw new Error("shared corpus must include not_found");
		const rawEnvelope = `${JSON.stringify(fixture, null, 2)}\n`;
		const result = await routeVerb(
			"state",
			undefined,
			["state", "pij-absent", "--json"],
			deps(answering(rawEnvelope, 404)),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", detail: fixture.meta });
		expect(result.outcome).not.toHaveProperty("code", "E-RS-UNPORTED");
		expect(result.rawEnvelope).toBe(rawEnvelope);
		expect(
			renderRsAnswer(result.row, result.payload, ["state", "--json"], result.rawEnvelope),
		).toEqual({ kind: "json", text: rawEnvelope });
	});

	it("no daemon is a named local refusal with the selected address", async () => {
		const result = await routeVerb(
			"adopt",
			undefined,
			["adopt"],
			deps(() => {
				throw connectionRefused();
			}),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED", addr: ADDR });
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			v: 2,
			details: { code: "E-RS-UNPORTED" },
		});
		expect(describeOutcome(result.outcome)).toContain(ADDR);
	});

	it.each([
		["adopt", undefined, { PIJ_DAEMON_GENERATION: "legacy" }],
		["spawn", undefined, {}],
		["register", undefined, {}],
		["unknown-command", undefined, {}],
	] as const)("%s %s refuses before reading credentials or making a request", async (verb, leaf, env) => {
		const calls: Call[] = [];
		let reads = 0;
		const result = await routeVerb(
			verb,
			leaf,
			leaf === undefined ? [verb] : [verb, leaf],
			deps(
				() => {
					throw new Error("unexpected request");
				},
				{
					env,
					readFile: (async () => {
						reads += 1;
						throw new Error("unexpected read");
					}) as unknown as GenerationRouterDeps["readFile"],
				},
				calls,
			),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			command: `pij ${verb}`,
			v: 2,
		});
		expect(calls).toEqual([]);
		expect(reads).toBe(0);
	});

	it.each([
		404, 405,
	])("a bodiless HTTP %s is unported and names the actual endpoint", async (status) => {
		const result = await routeVerb("adopt", undefined, ["adopt"], deps(answering("", status)));
		expect(result.outcome).toMatchObject({
			kind: "rs-error",
			code: "E-RS-UNPORTED",
			addr: ADDR,
			detail: expect.stringContaining("/v1/adopt"),
		});
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			details: { code: "E-RS-UNPORTED" },
		});
	});

	it("success preserves daemon-minted identity and the original response", async () => {
		const payload = { id: "pij-minted-by-rs" };
		const rawEnvelope = JSON.stringify({ ok: true, command: "pij adopt", v: 2, data: payload });
		const result = await routeVerb("adopt", undefined, ["adopt"], deps(answering(rawEnvelope)));
		expect(result.outcome).toMatchObject({ kind: "rs", addr: ADDR });
		expect(result.payload).toEqual(payload);
		expect(result.rawEnvelope).toBe(rawEnvelope);
	});

	it.each([
		// Plan 160: the shim always asks for each seat's size.
		["list", "/v1/seats?sizes=true", { seats: [{ id: "pij-rs-only" }] }],
		[
			"sessions",
			"/v1/shim/sessions",
			{
				rows: [
					{
						pijId: "pij-shared",
						generation: "rs",
						harnessSessionId: null,
						lifecycle: null,
						transcriptPath: null,
					},
				],
			},
		],
	] as const)("%s returns unmodified rs data with no registry union", async (verb, path, payload) => {
		const rawEnvelope = JSON.stringify({ ok: true, command: `pij ${verb}`, v: 2, data: payload });
		for (const argv of [[verb], [verb, "--json"]]) {
			const calls: Call[] = [];
			const result = await routeVerb(
				verb,
				undefined,
				argv,
				deps(
					(url, init) => {
						if (url.endsWith("/health")) return new Response(HEALTH);
						expect(init?.body).toBeUndefined();
						return new Response(rawEnvelope);
					},
					{},
					calls,
				),
			);
			expect(result.outcome.kind).toBe("rs");
			expect(calls).toEqual([
				{ url: `http://${ADDR}/health`, method: "GET" },
				{ url: `http://${ADDR}${path}`, method: "GET" },
			]);
			expect(result.payload).toEqual(payload);
			expect(result.rawEnvelope).toBe(rawEnvelope);
		}
	});

	it("list --here forwards the caller folder instead of refusing", async () => {
		const calls: Call[] = [];
		const folder = "/work/project with spaces";
		const result = await routeVerb(
			"list",
			undefined,
			["list", "--here", "--json"],
			deps(
				answering(
					JSON.stringify({
						ok: true,
						command: "pij seats",
						v: 2,
						data: { seats: [], unavailable: [] },
					}),
				),
				{ cwd: () => folder },
				calls,
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(new URL(calls[1]?.url ?? "").searchParams.get("here")).toBe(folder);
	});

	it.each([
		{ verb: "list", rejected: [["--role", "coder"], ["--all"], ["pij-other"]] },
		{
			verb: "sessions",
			rejected: [
				["--here"],
				["--all"],
				["--harness", "omp"],
				["--folder", "/tmp/pij-filter-fixture"],
				["--parent", "pij-parent"],
				["--scope", "local"],
				["pij-other"],
			],
		},
	])("$verb refuses unsupported filters instead of silently discarding them", async ({
		verb,
		rejected,
	}) => {
		for (const args of rejected) {
			const calls: Call[] = [];
			const result = await routeVerb(
				verb,
				undefined,
				[verb, ...args, "--json"],
				deps(
					() => {
						throw new Error("unexpected request");
					},
					{},
					calls,
				),
			);
			expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
			expect(describeOutcome(result.outcome)).toContain(verb);
			expect(calls).toEqual([]);
			expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
				ok: false,
				details: { code: "E-RS-UNPORTED" },
			});
		}
	});

	it("a transport failure after health is a request refusal", async () => {
		const result = await routeVerb(
			"adopt",
			undefined,
			["adopt"],
			deps((url) => {
				if (url.endsWith("/health")) return new Response(HEALTH);
				throw new Error("socket hang up");
			}),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", detail: "socket hang up" });
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			details: { code: "E-RS-REQUEST" },
		});
	});

	it("an undecodable operation response is not forwarded as a valid raw envelope", async () => {
		const result = await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(answering("<html>proxy failure</html>")),
		);
		expect(result.outcome.kind).toBe("rs-unreadable");
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			v: 2,
			details: { code: "E-RS-WIRE" },
		});
	});

	it("the resolved address controls both health and the actual call", async () => {
		const calls: Call[] = [];
		await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(
				answering('{"ok":true,"command":"pij whoami","v":2,"data":{"id":"pij-x"}}'),
				{ env: { PIJ_RS_ADDR: "127.0.0.1:18744" } },
				calls,
			),
		);
		expect(calls.map((call) => new URL(call.url).host)).toEqual([
			"127.0.0.1:18744",
			"127.0.0.1:18744",
		]);
	});
});

describe("credentials never authorize fallback", () => {
	it("a missing credential produces a named refusal, not an inferred residency", async () => {
		const result = await routeVerb("whoami", undefined, ["whoami"], keyless());
		expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
		const line = describeOutcome(result.outcome);
		expect(line).toContain("no rs credential");
		expect(line).toContain(`${STATE_DIR}/daemon.key`);
		expect(line).toContain(ADDR);
		expect(line).not.toMatch(/never registered/);
	});

	it("missing and rejected credentials have distinct outcomes and diagnostics", async () => {
		const absent = await routeVerb("whoami", undefined, ["whoami"], keyless());
		const rejected = await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(() => new Response(AUTH_401, { status: 401 })),
		);
		expect(rejected.outcome).toMatchObject({ kind: "rs-auth-rejected", stateDir: STATE_DIR });
		expect(absent.outcome.kind).not.toBe(rejected.outcome.kind);
		expect(describeOutcome(absent.outcome)).not.toBe(describeOutcome(rejected.outcome));
		expect(rejected.rawEnvelope).toBe(AUTH_401);
	});

	it.each([
		true,
		false,
	])("health authentication refusal retains exact bytes and details (stored key: %s)", async (hasKey) => {
		const rawEnvelope =
			' {"ok":false,"command":"pij ping","v":2,"error":"auth","meta":"key refused","details":{"code":"E-HEALTH-AUTH","evidence":{"retained":true}}}\n';
		const calls: Call[] = [];
		const result = await routeVerb(
			"whoami",
			undefined,
			["whoami", "--json"],
			deps(
				() => new Response(rawEnvelope, { status: 401 }),
				{
					readFile: (async () => {
						if (!hasKey) throw new Error("ENOENT");
						return KEY;
					}) as unknown as GenerationRouterDeps["readFile"],
				},
				calls,
			),
		);
		expect(result.outcome.kind).toBe(hasKey ? "rs-auth-rejected" : "rs-error");
		expect(result.rawEnvelope).toBe(rawEnvelope);
		expect(
			renderRsAnswer(result.row, result.payload, ["whoami", "--json"], result.rawEnvelope),
		).toEqual({ kind: "json", text: rawEnvelope });
		expect(calls.map((call) => new URL(call.url).pathname)).toEqual(["/health"]);
	});

	it("a synthetic missing-credential refusal never claims the successful health envelope", async () => {
		const result = await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(() => new Response(HEALTH), {
				readFile: (async () => {
					throw new Error("ENOENT");
				}) as unknown as GenerationRouterDeps["readFile"],
			}),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			command: "pij whoami",
			v: 2,
			details: { code: "E-RS-UNPORTED" },
		});
		expect(result.rawEnvelope).not.toBe(HEALTH);
	});
});

describe("literal body-file transport", () => {
	it.each([
		"/tmp/pij-router-body-fixture.txt",
		"-",
	])("reads %s literally without shell interpretation", async (path) => {
		const literal = "  $(touch never) `echo never`\nquotes: '\"; --json\n";
		const argv = ["send", "pij-target", "--body-file", path, "--json"];
		const reads: string[] = [];
		let request: unknown;
		const result = await routeVerb(
			"send",
			undefined,
			argv,
			deps(
				(url, init) => {
					if (url.endsWith("/health")) return new Response(HEALTH);
					request = JSON.parse(String(init?.body));
					return new Response(
						'{"ok":true,"command":"pij send","v":2,"data":{"msg_id":"literal-1"}}',
					);
				},
				{
					readBody: async (bodyPath) => {
						reads.push(bodyPath);
						return literal;
					},
				},
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(reads).toEqual([path]);
		expect(request).toEqual({ argv, caller: {}, body_literal: literal });
	});

	it.each([
		{ args: ["--body-file"] },
		{ args: ["--body-file", "--json"] },
	])("missing body path refuses without invoking send: $args", async ({ args }) => {
		const calls: Call[] = [];
		const result = await routeVerb(
			"send",
			undefined,
			["send", "pij-target", ...args],
			deps(() => new Response(HEALTH), {}, calls),
		);
		expect(result.outcome).toMatchObject({
			kind: "rs-error",
			detail: expect.stringContaining("takes a path"),
		});
		expect(calls.map((call) => new URL(call.url).pathname)).toEqual(["/health"]);
	});

	it("an unreadable body fails before sending, rather than sending an empty body", async () => {
		const calls: Call[] = [];
		const result = await routeVerb(
			"send",
			undefined,
			["send", "pij-target", "--body-file", "/tmp/pij-missing-body-fixture"],
			deps(
				() => new Response(HEALTH),
				{
					readBody: async () => {
						throw new Error("fixture ENOENT");
					},
				},
				calls,
			),
		);
		expect(result.outcome).toMatchObject({
			kind: "rs-error",
			detail: "--body-file: fixture ENOENT",
		});
		expect(calls.map((call) => new URL(call.url).pathname)).toEqual(["/health"]);
	});
});

describe("mailbox rendering precedes acknowledgement", () => {
	const claims = [
		{ job_id: 17, message: { from: "pij-peer", body: "one" } },
		{ job_id: "job-18", message: { from: "pij-peer", body: "two" } },
	];
	const rawEnvelope = JSON.stringify({ ok: true, command: "pij inbox", v: 2, data: claims });

	it("claims and rendering do not ack; the caller invokes the ordered ack closure after output", async () => {
		const events: string[] = [];
		const requests: unknown[] = [];
		const result = await routeVerb(
			"inbox",
			"check",
			["inbox", "check"],
			deps(
				(url, init) => {
					if (url.endsWith("/health")) return new Response(HEALTH);
					if (url.endsWith("/ack")) {
						events.push("ack");
						requests.push(JSON.parse(String(init?.body)));
						expect(init?.method).toBe("POST");
						expect(init?.headers).toMatchObject({ Authorization: `Bearer ${KEY}` });
						return new Response('{"ok":true,"command":"pij inbox ack","v":2,"data":{}}');
					}
					events.push("claim");
					return new Response(rawEnvelope);
				},
				{ env: { TMUX_PANE: "%ack-fixture" }, pid: () => 123, procStart: () => 20260907120000 },
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(events).toEqual(["claim"]);
		expect(renderRsAnswer(result.row, result.payload, ["inbox"]).kind).toBe("rendered");
		expect(events).toEqual(["claim"]);
		events.push("printed");
		expect(result.acknowledge).toBeTypeOf("function");
		expect(await result.acknowledge?.()).toBeUndefined();
		expect(events).toEqual(["claim", "printed", "ack", "ack"]);
		expect(requests).toEqual(
			claims.map(({ job_id }) => ({
				caller: { tmuxPane: "%ack-fixture", pid: 123, procStart: 20260907120000 },
				job_id,
			})),
		);
	});

	it("a render failure leaves claims unacknowledged", async () => {
		const calls: Call[] = [];
		const row = {
			verb: "inbox",
			rsPath: "/v1/shim/inbox",
			method: "POST",
			acknowledgePath: "/v1/shim/inbox/ack",
			why: "render failure fixture",
			render: () => {
				throw new Error("output failed");
			},
		} as const;
		const result = await routeVerb(
			"inbox",
			undefined,
			["inbox"],
			deps(answering(rawEnvelope), {}, calls),
			[row],
		);
		expect(result.acknowledge).toBeTypeOf("function");
		expect(() => renderRsAnswer(result.row, result.payload, ["inbox"])).toThrow("output failed");
		expect(calls.map((call) => new URL(call.url).pathname)).toEqual(["/health", "/v1/shim/inbox"]);
	});

	it.each([
		{ payload: [] },
		{ payload: [{ message: { body: "no claim identifier" } }] },
		{ payload: [{ job_id: null }] },
	])("does not offer an ack closure without valid claim ids: $payload", async ({ payload }) => {
		const result = await routeVerb(
			"inbox",
			undefined,
			["inbox"],
			deps(answering(JSON.stringify({ ok: true, command: "pij inbox", v: 2, data: payload }))),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(result.acknowledge).toBeUndefined();
	});

	it.each([
		"http",
		"wire",
		"transport",
	])("an ack %s failure warns about redelivery and stops the sequence", async (failure) => {
		let acks = 0;
		const result = await routeVerb(
			"inbox",
			undefined,
			["inbox"],
			deps((url) => {
				if (url.endsWith("/health")) return new Response(HEALTH);
				if (!url.endsWith("/ack")) return new Response(rawEnvelope);
				acks += 1;
				if (failure === "transport") throw new Error("socket closed");
				return failure === "http" ? new Response("", { status: 500 }) : new Response("<html>");
			}),
		);
		expect(await result.acknowledge?.()).toMatch(/acknowledgement failed.*may be read again/);
		expect(acks).toBe(1);
	});
});

describe("verified ambient native admission bridge", () => {
	const host = { pid: 9001, proc_start: 20260907120000 };
	const callerStart = 20260907120100;
	const fixtureIdentity = (harness: "claude" | "copilot" | "codex") => ({
		harness,
		harnessSessionId: "native-session",
	});
	const registered = (harness: string) =>
		JSON.stringify({
			ok: true,
			command: "pij register",
			v: 2,
			data: {
				id: "pij-ambient-host",
				harness,
				session: "native-session",
				pane: null,
				proc: host,
				native_extension_delivery: false,
				binding: "same",
			},
		});

	it.each([
		"claude",
		"copilot",
		"codex",
	] as const)("%s registration forwards evidence to the native allocator and retains the complete envelope", async (harness) => {
		const requests: unknown[] = [];
		const raw = registered(harness);
		const result = await routeVerb(
			"inbox",
			"register",
			["inbox", "register", "--json"],
			deps(
				(url, init) => {
					if (url.endsWith("/health")) return new Response(HEALTH);
					expect(new URL(url).pathname).toBe("/v1/register");
					requests.push(JSON.parse(String(init?.body)));
					return new Response(raw);
				},
				{
					ambientIdentity: () => fixtureIdentity(harness),
					cwd: () => "/work/external",
					pid: () => 9002,
					procStart: () => callerStart,
					env: { PIJ_SESSION_ID: "pij-stale", PIJ_PARENT_ID: "pij-parent" },
				},
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(requests).toEqual([
			{
				id: "",
				harness,
				harness_session: "native-session",
				folder: "/work/external",
				pid: 9002,
				proc_start: callerStart,
				relay: false,
				parent: "pij-parent",
			},
		]);
		expect(
			renderRsAnswer(
				result.row,
				result.payload,
				["inbox", "register", "--json"],
				result.rawEnvelope,
			),
		).toEqual({ kind: "json", text: raw });
		expect(result.acknowledge).toBeUndefined();
	});

	it("a repaired ambient identity and observed HOST tuple survive claim and post-output ack", async () => {
		const requests: { path: string; body: unknown }[] = [];
		const env = { PIJ_SESSION_ID: "pij-stale", COPILOT_AGENT_SESSION_ID: "native-session" };
		const claims = [
			{
				job_id: 71,
				message: {
					from: "pij-sender",
					body: "literal `$(text)`\nreply",
					msg_id: "m1",
					in_reply_to: "m0",
				},
			},
		];
		const result = await routeVerb(
			"inbox",
			undefined,
			["inbox", "--wait", "10", "--json"],
			deps(
				(url, init) => {
					const path = new URL(url).pathname;
					if (path === "/health") return new Response(HEALTH);
					requests.push({ path, body: JSON.parse(String(init?.body)) });
					if (path === "/v1/register") return new Response(registered("copilot"));
					return new Response(
						JSON.stringify({
							ok: true,
							command: "pij inbox",
							v: 2,
							data: path.endsWith("/ack") ? 72 : claims,
						}),
					);
				},
				{
					env,
					ambientIdentity: () => fixtureIdentity("copilot"),
					cwd: () => "/work/external",
					pid: () => 9002,
					procStart: () => callerStart,
				},
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(result.payload).toEqual(claims);
		expect(requests.map(({ path }) => path)).toEqual(["/v1/register", "/v1/shim/inbox"]);
		expect(await result.acknowledge?.()).toBeUndefined();
		const caller = {
			pijSessionId: "pij-ambient-host",
			copilotAgentSessionId: "native-session",
			cwd: "/work/external",
			pid: host.pid,
			procStart: host.proc_start,
		};
		expect(requests[1]?.body).toEqual({ argv: ["inbox", "--wait", "10", "--json"], caller });
		expect(requests[2]).toEqual({ path: "/v1/shim/inbox/ack", body: { caller, job_id: 71 } });
		expect(env.PIJ_SESSION_ID).toBe("pij-stale");
	});

	it.each([
		"send",
		"whoami",
		"phonehome",
	])("%s resolves ambient identity before forwarding its untouched argv", async (verb) => {
		const argv = verb === "send" ? [verb, "pij-other", "body text", "--in-reply-to", "m0"] : [verb];
		let request: unknown;
		const result = await routeVerb(
			verb,
			undefined,
			argv,
			deps(
				(url, init) => {
					if (url.endsWith("/health")) return new Response(HEALTH);
					if (url.endsWith("/register")) return new Response(registered("claude"));
					request = JSON.parse(String(init?.body));
					return new Response(JSON.stringify({ ok: true, command: `pij ${verb}`, v: 2, data: {} }));
				},
				{
					ambientIdentity: () => fixtureIdentity("claude"),
					cwd: () => "/work/external",
					pid: () => 9002,
					procStart: () => callerStart,
				},
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(request).toEqual({
			argv,
			caller: {
				pijSessionId: "pij-ambient-host",
				claudeCodeSessionId: "native-session",
				cwd: "/work/external",
				pid: host.pid,
				procStart: host.proc_start,
			},
		});
	});

	it("native admission refusal is terminal and preserved rather than using an inherited seat", async () => {
		const calls: Call[] = [];
		const raw =
			'{"ok":false,"command":"pij register","v":2,"error":"refused","meta":"host/session conflict"}\n';
		const result = await routeVerb(
			"send",
			undefined,
			["send", "pij-other", "hello"],
			deps(
				answering(raw, 400),
				{
					ambientIdentity: () => fixtureIdentity("claude"),
					env: { PIJ_SESSION_ID: "pij-stale" },
					cwd: () => "/work",
					pid: () => 9002,
					procStart: () => callerStart,
				},
				calls,
			),
		);
		expect(result.outcome.kind).toBe("rs-error");
		expect(result.rawEnvelope).toBe(raw);
		expect(calls.map(({ url }) => new URL(url).pathname)).toEqual(["/health", "/v1/register"]);
	});

	it("already registered panes read whoami without ambient discovery or native allocation", async () => {
		const calls: Call[] = [];
		const raw =
			'{"ok":true,"command":"pij whoami","v":2,"data":{"id":"pij-pane","harness":"omp","pane":"%44"}}';
		const result = await routeVerb(
			"inbox",
			"register",
			["inbox", "register"],
			deps(
				answering(raw),
				{
					env: { TMUX_PANE: "%44" },
					ambientIdentity: () => {
						throw new Error("pane must not discover ambient files");
					},
				},
				calls,
			),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(result.rawEnvelope).toBe(raw);
		expect(calls.map(({ url }) => new URL(url).pathname)).toEqual(["/health", "/v1/whoami"]);
		expect(renderRsAnswer(result.row, result.payload, ["inbox", "register"]).kind).toBe("rendered");
	});

	it("register without ambient or registered identity refuses instead of posting an empty native claim", async () => {
		const calls: Call[] = [];
		const result = await routeVerb(
			"inbox",
			"register",
			["inbox", "register", "--json"],
			deps(answering("{}"), { ambientIdentity: () => null }, calls),
		);
		expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-AMBIG" });
		expect(calls.map(({ url }) => new URL(url).pathname)).toEqual(["/health"]);
	});
});
