import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
	buildCallerContext,
	CALLER_ENV_ALLOWLIST,
	CALLER_WIRE_KEYS,
	describeOutcome,
	findRoute,
	RS_ROUTE_TABLE,
	type RsHttpRoute,
	renderRsAnswer,
} from "../core/generation-routing.js";
import { type GenerationRouterDeps, probeDecisionOnly, routeVerb } from "./generation-router.js";

const KEY = "k".repeat(64);
const ADDR = "127.0.0.1:18745";
const STATE_DIR = "/tmp/pij-generation-seam-fixture";
const HEALTH = JSON.stringify({
	ok: true,
	command: "pij ping",
	v: 2,
	data: { status: "healthy", build: "fixture", offline: false, machine: "fixture" },
});

interface Seen {
	readonly url: string;
	readonly method: string;
	readonly body?: string | undefined;
}

function deps(seen: Seen[], over: Partial<GenerationRouterDeps> = {}): GenerationRouterDeps {
	return {
		fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
			const url = String(input);
			if (url.endsWith("/health")) return new Response(HEALTH);
			seen.push({ url, method: init?.method ?? "GET", body: init?.body as string | undefined });
			return new Response(
				JSON.stringify({
					ok: true,
					command: `pij ${new URL(url).pathname.split("/").at(-1)}`,
					v: 2,
					data: {},
				}),
			);
		}) as unknown as typeof fetch,
		readFile: (async (path: unknown) => {
			if (String(path) !== `${STATE_DIR}/daemon.key`)
				throw new Error(`unexpected file read: ${String(path)}`);
			return KEY;
		}) as unknown as GenerationRouterDeps["readFile"],
		readBody: async (path) => {
			throw new Error(`unexpected body read: ${path}`);
		},
		home: "/tmp/pij-generation-seam-home-fixture",
		...over,
		env: { PIJ_RS_ADDR: ADDR, PIJ_RS_STATE_DIR: STATE_DIR, ...over.env },
	};
}

const CALLER_ENV = {
	PIJ_SESSION_ID: "pij-fixture-caller",
	TMUX_PANE: "%2088",
	CLAUDE_CODE_SESSION_ID: "191b35a2-6ecb",
};

describe("caller evidence crosses the actual HTTP seam", () => {
	it.each([
		"whoami",
		"state",
		"phonehome",
	])("%s carries caller identity in a POST body", async (verb) => {
		const seen: Seen[] = [];
		const result = await routeVerb(verb, undefined, [verb], deps(seen, { env: CALLER_ENV }));
		expect(result.outcome.kind).toBe("rs");
		expect(seen).toHaveLength(1);
		expect(seen[0]).toMatchObject({ url: `http://${ADDR}/v1/${verb}`, method: "POST" });
		expect(JSON.parse(seen[0]?.body ?? "null")).toEqual({
			argv: [verb],
			caller: {
				pijSessionId: "pij-fixture-caller",
				tmuxPane: "%2088",
				claudeCodeSessionId: "191b35a2-6ecb",
			},
		});
	});

	it("a positional subject and flags reach the daemon without client-side residency decisions", async () => {
		const seen: Seen[] = [];
		const argv = ["state", "--json", "pij-other"];
		await routeVerb("state", undefined, argv, deps(seen, { env: CALLER_ENV }));
		expect(JSON.parse(seen[0]?.body ?? "null")).toMatchObject({
			argv,
			caller: { pijSessionId: "pij-fixture-caller" },
		});
		expect(seen.map((call) => new URL(call.url).pathname)).toEqual(["/v1/state"]);
	});

	it("only list and sessions use GET; caller-bound operations stay POST", () => {
		expect(
			RS_ROUTE_TABLE.filter((row) => "rsPath" in row && row.method === "GET").map(
				(row) => row.verb,
			),
		).toEqual(["list", "sessions"]);
	});

	it.each([
		"anomalies",
		"decisions",
	])("%s forwards argv and caller to its existing POST endpoint", async (verb) => {
		const seen: Seen[] = [];
		const argv = [verb, "--json"];
		await routeVerb(verb, undefined, argv, deps(seen, { env: CALLER_ENV }));
		expect(seen[0]).toMatchObject({ url: `http://${ADDR}/v1/${verb}`, method: "POST" });
		expect(JSON.parse(seen[0]?.body ?? "null")).toMatchObject({
			argv,
			caller: { tmuxPane: "%2088" },
		});
	});
});

describe("the caller block is an allowlist, never an environment dump", () => {
	it("carries allowlisted fields and observed process identity", () => {
		expect(
			buildCallerContext(
				{ PIJ_SESSION_ID: "pij-a", TMUX_PANE: "%1", CODEX_THREAD_ID: "t-9" },
				{ cwd: "/w", pid: 42, procStart: 20260901110000 },
			),
		).toEqual({
			pijSessionId: "pij-a",
			tmuxPane: "%1",
			codexThreadId: "t-9",
			cwd: "/w",
			pid: 42,
			procStart: 20260901110000,
		});
	});

	it("does not carry secrets or any field outside the allowlist", () => {
		const caller = buildCallerContext(
			{
				PIJ_SESSION_ID: "pij-a",
				ANTHROPIC_API_KEY: "sk-secret",
				GITHUB_TOKEN: "ghp-secret",
				AWS_SECRET_ACCESS_KEY: "aws-secret",
				HOME: "/Users/fixture",
			},
			{},
		);
		expect(JSON.stringify(caller)).not.toContain("secret");
		expect(caller).toEqual({ pijSessionId: "pij-a" });
	});

	it("non-allowlisted values never reach the wire", async () => {
		const seen: Seen[] = [];
		await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(seen, { env: { ...CALLER_ENV, ANTHROPIC_API_KEY: "sk-must-not-travel" } }),
		);
		expect(seen).toHaveLength(1);
		expect(seen[0]?.body).not.toContain("sk-must-not-travel");
	});

	it("absent and empty environment fields are omitted, not null-filled", () => {
		expect(buildCallerContext({ PIJ_SESSION_ID: "pij-a", TMUX_PANE: "" }, {})).toEqual({
			pijSessionId: "pij-a",
		});
	});

	it("a process start that cannot be observed is omitted, not invented", async () => {
		const seen: Seen[] = [];
		await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(seen, {
				env: CALLER_ENV,
				pid: () => 4285,
				procStart: () => {
					throw new Error("ps failed");
				},
			}),
		);
		const body = JSON.parse(seen[0]?.body ?? "null");
		expect(body.caller.pid).toBe(4285);
		expect(body.caller).not.toHaveProperty("procStart");
	});

	it("the allowlist names every environment field deliberately", () => {
		expect(CALLER_ENV_ALLOWLIST.map(([name]) => name)).toEqual([
			"PIJ_SESSION_ID",
			"TMUX_PANE",
			"PIJ_PARENT_ID",
			"CLAUDE_CODE_SESSION_ID",
			"COPILOT_AGENT_SESSION_ID",
			"CODEX_THREAD_ID",
		]);
	});
});

describe("cross-runtime caller-key fixture", () => {
	const fixture = new URL(
		"../../../../crates/daemon/tests/fixtures/caller-context.wire.json",
		import.meta.url,
	);
	const wireContract = (): Record<string, unknown> => {
		const contract = JSON.parse(readFileSync(fixture, "utf8")) as Record<string, unknown>;
		delete contract._comment;
		return contract;
	};
	const fixtureEnv = (contract: Record<string, unknown>) => ({
		PIJ_SESSION_ID: contract.pijSessionId as string,
		TMUX_PANE: contract.tmuxPane as string,
		PIJ_PARENT_ID: contract.pijParentId as string,
		CLAUDE_CODE_SESSION_ID: contract.claudeCodeSessionId as string,
		COPILOT_AGENT_SESSION_ID: contract.copilotAgentSessionId as string,
		CODEX_THREAD_ID: contract.codexThreadId as string,
	});

	it("the shim writes exactly the populated contract Rust consumes", () => {
		const contract = wireContract();
		expect(
			buildCallerContext(fixtureEnv(contract), {
				cwd: contract.cwd as string,
				pid: contract.pid as number,
				procStart: contract.procStart as number,
			}),
		).toEqual(contract);
	});

	it("the actual POST carries exactly that contract", async () => {
		const contract = wireContract();
		const seen: Seen[] = [];
		await routeVerb(
			"whoami",
			undefined,
			["whoami"],
			deps(seen, {
				env: fixtureEnv(contract),
				cwd: () => contract.cwd as string,
				pid: () => contract.pid as number,
				procStart: () => contract.procStart as number,
			}),
		);
		expect(JSON.parse(seen[0]?.body ?? "null").caller).toEqual(contract);
	});

	it("the shared fixture populates every wire key, preventing vacuous equality", () => {
		const contract = wireContract();
		expect(Object.keys(contract).sort()).toEqual([...CALLER_WIRE_KEYS].sort());
		for (const [key, value] of Object.entries(contract)) {
			expect(value, key).not.toBeUndefined();
			expect(value, key).not.toBeNull();
			expect(String(value).length, key).toBeGreaterThan(0);
		}
		for (const [, field] of CALLER_ENV_ALLOWLIST) expect(CALLER_WIRE_KEYS).toContain(field);
	});
});

describe("leaf routing leaves grammar with the daemon", () => {
	it.each([
		"now",
		"state",
		"blocked",
		"clear",
		"question",
		"verify",
	])("report %s uses the shared report endpoint, never an invented leaf path", async (leaf) => {
		const seen: Seen[] = [];
		const argv = ["report", leaf, "--json"];
		await routeVerb("report", leaf, argv, deps(seen));
		expect(seen[0]).toMatchObject({ url: `http://${ADDR}/v1/report`, method: "POST" });
		expect(JSON.parse(seen[0]?.body ?? "null").argv).toEqual(argv);
	});

	it.each(["frobnicate"])("inbox %s is a named refusal, not a bare-inbox read", async (leaf) => {
		const seen: Seen[] = [];
		const result = await routeVerb("inbox", leaf, ["inbox", leaf], deps(seen));
		expect(result.outcome).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
		expect(JSON.parse(result.rawEnvelope ?? "null")).toMatchObject({
			ok: false,
			details: { code: "E-RS-UNPORTED" },
		});
		expect(seen).toEqual([]);
	});

	it.each([
		undefined,
		"check",
	])("inbox %s still reads on pane evidence without inventing a session id", async (leaf) => {
		const seen: Seen[] = [];
		const argv = leaf === undefined ? ["inbox"] : ["inbox", leaf];
		const result = await routeVerb(
			"inbox",
			leaf,
			argv,
			deps(seen, { env: { TMUX_PANE: "%2159" } }),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(seen).toHaveLength(1);
		expect(seen[0]?.url).toBe(`http://${ADDR}/v1/shim/inbox`);
		expect(JSON.parse(seen[0]?.body ?? "null")).toEqual({ argv, caller: { tmuxPane: "%2159" } });
	});

	it("send forwards only the pane it has and never looks up another registry", async () => {
		const seen: Seen[] = [];
		const result = await routeVerb(
			"send",
			undefined,
			["send", "pij-peer", "hello"],
			deps(seen, { env: { TMUX_PANE: "%2159" } }),
		);
		expect(result.outcome.kind).toBe("rs");
		expect(seen.map((call) => new URL(call.url).pathname)).toEqual(["/v1/shim/send"]);
		expect(JSON.parse(seen[0]?.body ?? "null").caller).toEqual({ tmuxPane: "%2159" });
	});
});

describe("generation diagnostics report capability attempts, not completed operations", () => {
	it("expands actual inbox leaves without performing registration", async () => {
		const seen: Seen[] = [];
		const decisions = await probeDecisionOnly(["inbox"], deps(seen));
		expect(decisions.map((decision) => decision.verb)).toEqual([
			"inbox",
			"inbox check",
			"inbox register",
		]);
		expect(decisions.map((decision) => decision.generation)).toEqual(["rs", "rs", "rs"]);
		expect(decisions[2]?.line).toContain("/v1/register");
		expect(seen).toEqual([]);
	});

	it("supported HTTP and native rows are would-attempt diagnostics without performing either", async () => {
		const seen: Seen[] = [];
		let nativeCalls = 0;
		const decisions = await probeDecisionOnly(
			["report", "commit-trailers"],
			deps(seen, {
				runNative: async () => {
					nativeCalls += 1;
					return 7;
				},
			}),
		);
		expect(decisions.map((decision) => decision.verb)).toEqual(["report", "commit-trailers"]);
		for (const decision of decisions) {
			expect(decision.generation).toBe("rs");
			expect(decision.line).toContain("would be attempted");
			expect(decision.line).toContain("no operation was performed");
			expect(decision.addr).toBe(ADDR);
		}
		expect(decisions[0]?.line).toContain("/v1/report");
		expect(decisions[1]?.line).toContain("pij-rs commit-trailers");
		expect(seen).toEqual([]);
		expect(nativeCalls).toBe(0);
	});

	it("forced-legacy diagnostics agree with actual refusal, without touching the network", async () => {
		let requests = 0;
		const shared = deps([], {
			env: { PIJ_DAEMON_GENERATION: "legacy" },
			fetch: (async () => {
				requests += 1;
				throw new Error("unexpected request");
			}) as unknown as typeof fetch,
		});
		const [diagnosed] = await probeDecisionOnly(["whoami"], shared);
		const result = await routeVerb("whoami", undefined, ["whoami"], shared);
		expect(diagnosed?.generation).toBe("rs-failed");
		expect(diagnosed?.line).toBe(describeOutcome(result.outcome));
		expect(requests).toBe(0);
	});

	it("native-only and unported diagnostics require no health probe", async () => {
		let requests = 0;
		const decisions = await probeDecisionOnly(
			["commit-trailers", "tail", "unknown-command"],
			deps([], {
				fetch: (async () => {
					requests += 1;
					throw new Error("unexpected request");
				}) as unknown as typeof fetch,
			}),
		);
		expect(decisions.map((decision) => decision.generation)).toEqual([
			"rs",
			"rs-failed",
			"rs-failed",
		]);
		expect(requests).toBe(0);
	});
});

// This is a source-backed endpoint witness, not a substitute for PM's daemon smoke.
// The positive control is the original report/now false path that once passed mock tests.
describe("every HTTP and acknowledgement path exists in the actual daemon", () => {
	const source = () =>
		readFileSync(new URL("../../../../crates/daemon/src/http/mod.rs", import.meta.url), "utf8");
	const registeredPaths = (rust: string): ReadonlySet<string> => {
		const paths = new Set<string>();
		for (const match of rust.matchAll(/Self::\w+\s*=>\s*"(\/v1\/[^"]*|\/health)"/g))
			paths.add(match[1] as string);
		if (paths.size === 0) throw new Error("no endpoint paths found — did Endpoint::path move?");
		return paths;
	};

	it("every declared HTTP path and ack path is registered", () => {
		const registered = registeredPaths(source());
		const missing: string[] = [];
		for (const row of RS_ROUTE_TABLE) {
			if (!("rsPath" in row)) continue;
			for (const path of [row.rsPath, row.acknowledgePath]) {
				if (path !== undefined && !registered.has(path))
					missing.push(`${row.verb} ${row.leaf ?? ""}: ${path}`);
			}
		}
		expect(missing).toEqual([]);
	});

	it("detects the report leaf path the daemon never registered", () => {
		const registered = registeredPaths(source());
		expect(registered.has("/v1/report")).toBe(true);
		expect(registered.has("/v1/report/now")).toBe(false);
		expect(RS_ROUTE_TABLE.filter((row) => row.verb === "report")).toEqual([
			expect.objectContaining({ rsPath: "/v1/report", method: "POST" }),
		]);
	});

	it("mailbox acknowledgements use the shim caller-evidence protocol", () => {
		for (const leaf of [undefined, "check"])
			expect(findRoute("inbox", leaf)).toMatchObject({ acknowledgePath: "/v1/shim/inbox/ack" });
		expect(findRoute("send", undefined)).not.toHaveProperty("acknowledgePath");
	});
});

describe("human rendering and original-envelope JSON", () => {
	const bare: RsHttpRoute = {
		verb: "report",
		rsPath: "/v1/report",
		method: "POST",
		why: "renderer fixture",
	};
	const human = (verb: string, payload: unknown) =>
		renderRsAnswer(findRoute(verb, undefined), payload, [verb]);

	it("explicit row renderers take precedence over a daemon line", () => {
		expect(
			renderRsAnswer({ ...bare, render: () => "custom output" }, { line: "daemon output" }, [
				"report",
			]),
		).toEqual({ kind: "rendered", text: "custom output" });
	});

	it("unrendered HTTP rows prefer the daemon's own sentence", () => {
		expect(
			renderRsAnswer(bare, { line: 'reported by pij-x: "did" -> "next" (spine 42)' }, [
				"report",
				"now",
			]),
		).toEqual({ kind: "rendered", text: 'reported by pij-x: "did" -> "next" (spine 42)' });
	});

	it.each([
		{ a: 1 },
		{ line: { not: "a string" } },
		{ line: "" },
	])("other payloads remain readable JSON, without treating a non-string as a sentence", (payload) => {
		expect(renderRsAnswer(bare, payload, ["report"])).toEqual({
			kind: "rendered",
			text: JSON.stringify(payload, null, 2),
		});
	});

	it("--json requires original bytes and bypasses human rendering", () => {
		const row: RsHttpRoute = {
			...bare,
			render: () => {
				throw new Error("human renderer must not run");
			},
		};
		const rawEnvelope =
			' {"ok":true,"command":"pij report","v":2,"data":{"line":"done","extra":true}}\n';
		expect(renderRsAnswer(row, { line: "done" }, ["report", "--json"], rawEnvelope)).toEqual({
			kind: "json",
			text: rawEnvelope,
		});
		expect(renderRsAnswer(row, { line: "done" }, ["report", "--json"]).kind).toBe("json-refused");
	});

	it("every HTTP row can render a response and pass through complete JSON", () => {
		for (const row of RS_ROUTE_TABLE) {
			if (!("rsPath" in row)) continue;
			const payload = { line: "daemon sentence", rsOnlyKey: "retained" };
			const argv = row.leaf === undefined ? [row.verb] : [row.verb, row.leaf];
			const rawEnvelope = JSON.stringify({
				ok: true,
				command: `pij ${row.verb}`,
				v: 2,
				data: payload,
			});
			expect(renderRsAnswer(row, payload, argv).kind, argv.join(" ")).toBe("rendered");
			expect(renderRsAnswer(row, payload, [...argv, "--json"], rawEnvelope)).toEqual({
				kind: "json",
				text: rawEnvelope,
			});
		}
	});

	it("native and refusal rows do not invent HTTP rendering", () => {
		for (const row of RS_ROUTE_TABLE) {
			if ("rsPath" in row) continue;
			expect(renderRsAnswer(row, {}, [row.verb]).kind).toBe("no-renderer");
		}
	});

	it("adopt reports the admitted id, pane and process binding, never an invented harness session", () => {
		const result = human("adopt", {
			id: "pij-adopt-fixture",
			harness: "claude",
			pane: "%2099",
			proc: { pid: 4197, proc_start: 20260901130939 },
			folder: "/w",
			state: "idle",
		});
		if (result.kind !== "rendered") throw new Error("adopt must render");
		expect(result.text).toContain("adopted pij-adopt-fixture");
		expect(result.text).toContain("%2099");
		expect(result.text).toContain("process identity");
		expect(result.text).toContain('pij send pij-adopt-fixture "<text>"');
		expect(result.text).not.toMatch(/session\s+[0-9a-f-]{8}/);
	});

	it("paneless and unbound adoption states are explicit", () => {
		const result = human("adopt", { id: "pij-x" });
		if (result.kind !== "rendered") throw new Error("adopt must render");
		expect(result.text).toContain("no pane");
		expect(result.text).toContain("not bound to a process");
	});

	it("whoami, phonehome and state remain human-readable", () => {
		expect(human("whoami", { id: "pij-x", folder: "/w", state: "working" }).kind).toBe("rendered");
		expect(human("phonehome", { seat: "pij-x", harness: "claude", bound: true }).kind).toBe(
			"rendered",
		);
		expect(human("state", { id: "pij-x", state: "working", liveness: "active" }).kind).toBe(
			"rendered",
		);
	});

	it("unbound phonehome explains the daemon's actual evidence", () => {
		const result = human("phonehome", {
			seat: "pij-x",
			harness: "claude",
			bound: false,
			resolved_by: "asserted seat id (existence checked only)",
		});
		if (result.kind !== "rendered") throw new Error("phonehome must render");
		expect(result.text).toContain("NOT bound");
		expect(result.text).toContain("asserted seat id");
	});

	it("sessions renders the rs rows object without a legacy array projection", () => {
		const result = human("sessions", {
			rows: [{ pijId: "pij-rs", generation: "rs", harness: "omp", harnessSessionId: null }],
		});
		if (result.kind !== "rendered") throw new Error("sessions must render");
		expect(result.text).toContain("pij-rs  rs  omp  —");
		expect(result.text).toContain("1 session(s)");
		expect(human("sessions", { rows: [] })).toEqual({ kind: "rendered", text: "no pij sessions" });
	});
});

describe("CLI routing entry source witness", () => {
	it("the actual entry promise is caught rather than dropped", () => {
		const cli = readFileSync(new URL("../cli.ts", import.meta.url), "utf8");
		expect(cli).toMatch(/bootThroughGenerationRouting\(\)\s*\.catch\(/);
		expect(cli).not.toMatch(/void bootThroughGenerationRouting\(\)/);
	});
});
