// Plan 158 — the Pi/OMP extension's `fyi` and turn-activity behaviour, observed
// at the daemon wire and the host surfaces: the pij_send tool,
// before_agent_start, the `✉N` footer, and turn_start/turn_end publication.
// Plan 157 phase 2 — the pij_send tool's side of the cold-wake guard.
//
// The FYI block format belongs to the daemon (the golden fixture). These tests
// read the fixture and prove byte-for-byte pass-through; they never restate it.

import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as daemonHttp from "./adapters/daemon-http.js";
import pijExtension from "./index.js";

const GOLDEN_BLOCK = readFileSync(
	new URL("../../../crates/testkit/fixtures/golden/fyi/block.txt", import.meta.url),
	"utf8",
);

type Handler = (...args: unknown[]) => unknown;
type Claim = { count: number; block: string } | "refused" | "down";
interface Recorded {
	readonly path: string;
	readonly body: Record<string, unknown>;
}

interface FakeDaemon {
	readonly state: {
		pendingFyis: number;
		seat: string;
		/** How /v1/activity answers: applied, connection refused, or a route-absent 404. */
		activity: "up" | "down" | "absent";
	};
	readonly requests: Recorded[];
	/** Activity states in the order the daemon finished applying them. */
	readonly applied: string[];
	readonly generation: daemonHttp.DaemonGeneration;
	pushEvent(kind: string, seat: string): void;
}

interface Seat {
	readonly injected: string[];
	fyiFooter(): Array<string | undefined>;
	send(params: Record<string, unknown>): Promise<unknown>;
	beforeAgentStart(): Promise<unknown>;
	/** before_agent_start from an OMP in-process subagent sharing this extension. */
	embeddedChildBeforeAgentStart(): Promise<unknown>;
	turnStart(): Promise<unknown>;
	turnEnd(): Promise<unknown>;
	agentEnd(willContinue: boolean): Promise<unknown>;
	shutdown(): Promise<unknown>;
}

function envelope(command: string, data: unknown): Response {
	return new Response(JSON.stringify({ ok: true, command, v: 2, data }));
}

/** A Rust daemon double serving the contract's shapes and recording every request. */
function fakeDaemon(options: {
	pendingFyis: number;
	claim?: Claim;
	/** Holds the first /v1/activity response until the promise settles. */
	firstActivityGate?: Promise<void>;
	/** Answers /v1/send instead of the default accepting receipt. */
	send?: (body: Record<string, unknown>) => Response;
}): FakeDaemon {
	const state: FakeDaemon["state"] = {
		pendingFyis: options.pendingFyis,
		seat: "",
		activity: "up",
	};
	const requests: Recorded[] = [];
	const applied: string[] = [];
	let events: ReadableStreamDefaultController<Uint8Array> | undefined;
	const encoder = new TextEncoder();
	const fetchMock = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
		const path = new URL(String(url)).pathname;
		const body =
			typeof init?.body === "string" ? (JSON.parse(init.body) as Record<string, unknown>) : {};
		requests.push({ path, body });
		if (path === "/v1/seats") return envelope("pij seats", { seats: [], unavailable: [] });
		if (path === "/v1/inbox") return envelope("pij inbox", []);
		if (path === "/v1/register") {
			state.seat = String(body.id);
			return envelope("pij register", {
				id: body.id,
				harness: body.harness,
				proc: { pid: body.pid, proc_start: body.proc_start },
				folder: body.folder,
				state: "idle",
			});
		}
		if (path === "/v1/events") {
			return new Response(
				new ReadableStream<Uint8Array>({
					start(controller) {
						events = controller;
						controller.enqueue(
							encoder.encode(`${JSON.stringify({ hello: true, v: 1, build: "pij-rs test" })}\n`),
						);
					},
				}),
			);
		}
		if (path === "/v1/state") {
			return envelope("pij state", { id: body.id, pendingFyis: state.pendingFyis });
		}
		if (path === "/v1/send") {
			if (options.send) return options.send(body);
			return envelope("pij send", {
				msg_id: body.msg_id,
				outcome: body.fyi === true ? { outcome: "held", reason: "fyi" } : "accepted",
				at: 1,
			});
		}
		if (path === "/v1/fyi/claim") {
			if (options.claim === "down")
				throw Object.assign(new Error("connect ECONNREFUSED"), { code: "ECONNREFUSED" });
			if (options.claim === "refused" || options.claim === undefined)
				return new Response(
					JSON.stringify({
						ok: false,
						command: "pij fyi-claim",
						v: 2,
						error: "refused",
						meta: "no",
					}),
				);
			return envelope("pij fyi-claim", { seat: state.seat, ...options.claim, ids: [] });
		}
		if (path === "/v1/activity") {
			if (state.activity === "down")
				throw Object.assign(new Error("connect ECONNREFUSED"), { code: "ECONNREFUSED" });
			if (state.activity === "absent") return new Response("Not Found", { status: 404 });
			const first = requests.filter((request) => request.path === "/v1/activity").length === 1;
			if (first && options.firstActivityGate) await options.firstActivityGate;
			applied.push(String(body.state));
			return envelope("pij activity", { seat: body.seat, state: body.state, changed: true });
		}
		throw new Error(`unexpected Rust fixture path ${path}`);
	});
	const location = { addr: "127.0.0.1:7461", stateDir: "/unused" };
	return {
		state,
		requests,
		applied,
		generation: {
			kind: "rust",
			location,
			health: { status: "healthy", build: "pij-rs test", offline: true, machine: "test" },
			client: new daemonHttp.PijDaemonClient(location, "secret", {
				fetch: fetchMock as typeof fetch,
				readFile: async () => "secret",
				processStart: () => 20260929140000,
			}),
		},
		pushEvent(kind, seat) {
			const event = { v: 1, at: 1, kind, seat, payload: "{}" };
			const frame = { machine: "test", cursor: requests.length, event };
			events?.enqueue(encoder.encode(`${JSON.stringify(frame)}\n`));
		},
	};
}

async function bootSeat(daemon: FakeDaemon): Promise<Seat> {
	vi.spyOn(daemonHttp, "detectDaemonGeneration").mockResolvedValue(daemon.generation);
	const handlers = new Map<string, Handler>();
	const tools = new Map<string, { execute: Handler }>();
	const injected: string[] = [];
	const pi = {
		on: (event: string, handler: Handler) => handlers.set(event, handler),
		registerTool: (tool: { name: string; execute: Handler }) => tools.set(tool.name, tool),
		registerCommand: () => {},
		events: { on: () => {}, emit: () => {} },
		sendUserMessage: (message: string) => injected.push(message),
		sendMessage: (message: { content: string }) => injected.push(message.content),
		setSessionName: async () => {},
		getSessionName: () => undefined,
	} as unknown as ExtensionAPI;
	const statuses: Array<{ key: string; value: string | undefined }> = [];
	const ctx = {
		hasUI: true,
		mode: "tui",
		cwd: process.cwd(),
		sessionManager: { getSessionId: () => "native-fyi-session", getEntries: () => [] },
		isIdle: () => true,
		compact: () => {},
		ui: {
			setStatus: (key: string, value: string | undefined) => statuses.push({ key, value }),
			notify: () => {},
		},
	} as unknown as ExtensionContext;
	pijExtension(pi);
	await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
	const fire = async (event: string, payload: Record<string, unknown>) =>
		handlers.get(event)?.({ type: event, ...payload }, ctx);
	return {
		injected,
		fyiFooter: () => statuses.filter((status) => status.key === "pij-fyi").map((s) => s.value),
		send: async (params) =>
			tools.get("pij_send")?.execute("call", params, undefined, undefined, ctx),
		beforeAgentStart: () => fire("before_agent_start", { prompt: "hi" }),
		embeddedChildBeforeAgentStart: async () =>
			handlers.get("before_agent_start")?.(
				{ type: "before_agent_start", prompt: "child task" },
				{
					...ctx,
					mode: "print",
					hasUI: false,
					sessionManager: {
						getSessionId: () => "embedded-child-session",
						getEntries: () => [{ type: "session_init" }],
					},
				},
			),
		turnStart: () => fire("turn_start", { turnIndex: 0, timestamp: Date.now() }),
		turnEnd: () => fire("turn_end", { turnIndex: 0 }),
		agentEnd: (willContinue) => fire("agent_end", { messages: [], willContinue }),
		shutdown: () => fire("session_shutdown", { reason: "quit" }),
	};
}

const pathed = (daemon: FakeDaemon, path: string) =>
	daemon.requests.filter((request) => request.path === path);

describe("pij extension — fyi and turn activity (plan 158)", () => {
	let pijHome: string;
	let seat: Seat | undefined;

	beforeEach(() => {
		pijHome = mkdtempSync(join(tmpdir(), "pij-fyi-test-"));
		vi.stubEnv("PIJ_HOME", pijHome);
		vi.stubEnv("PIJ_SESSION_ID", undefined);
		vi.stubEnv("PIJ_ANNOUNCE_TO", "");
		vi.stubEnv("PIJ_SPAWN_TASK", undefined);
		vi.stubEnv("PIJ_PARENT_ID", undefined);
		vi.stubEnv("PIJ_PLAN_ID", undefined);
		vi.stubEnv("OMPCODE", undefined);
	});

	afterEach(async () => {
		await seat?.shutdown();
		seat = undefined;
		vi.restoreAllMocks();
		vi.unstubAllEnvs();
		rmSync(pijHome, { recursive: true, force: true });
	});

	describe("pij_send", () => {
		it("puts fyi:true on the /v1/send wire only when asked", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			seat = await bootSeat(daemon);
			await seat.send({ to: "pij-peer", message: "plain" });
			await seat.send({ to: "pij-peer", message: "heads up", fyi: true });
			expect(pathed(daemon, "/v1/send").map((request) => request.body.fyi)).toEqual([
				undefined,
				true,
			]);
		});

		it("renders the held receipt as held (fyi)", async () => {
			seat = await bootSeat(fakeDaemon({ pendingFyis: 0 }));
			const result = (await seat.send({ to: "pij-peer", message: "heads up", fyi: true })) as {
				content: Array<{ text: string }>;
			};
			expect(result.content[0]?.text).toMatch(/^held \(fyi\) .+ -> pij-peer$/);
		});

		it("refuses fyi with a control command before reaching the daemon", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			seat = await bootSeat(daemon);
			await expect(seat.send({ to: "pij-peer", command: "compact", fyi: true })).rejects.toThrow(
				"fyi",
			);
			expect(pathed(daemon, "/v1/send")).toEqual([]);
		});

		describe("cold-wake guard (plan 157 phase 2)", () => {
			const COLD_META =
				'E-RS-COLD-WAKE: pij-peer is cold (idle 1h52m, 720k context). Waking it rewrites ~720k tokens ≈ $5.76 at list price. Use --fyi to hold it, or --force --reason "<why>".';

			it("puts force and reason on the /v1/send wire", async () => {
				const daemon = fakeDaemon({ pendingFyis: 0 });
				seat = await bootSeat(daemon);
				await seat.send({
					to: "pij-peer",
					message: "wake",
					force: true,
					reason: "release blocker",
				});
				expect(pathed(daemon, "/v1/send")[0]?.body).toMatchObject({
					force: true,
					reason: "release blocker",
				});
			});

			it.each([
				["no reason", {}],
				["a blank reason", { reason: "   " }],
			])("refuses force with %s before any request", async (_label, extra) => {
				const daemon = fakeDaemon({ pendingFyis: 0 });
				seat = await bootSeat(daemon);
				await expect(
					seat.send({ to: "pij-peer", message: "wake", force: true, ...extra }),
				).rejects.toThrow(/^E-RS-COLD-WAKE: .*reason/);
				expect(pathed(daemon, "/v1/send")).toEqual([]);
			});

			it("surfaces the daemon's cold refusal verbatim as the tool error", async () => {
				seat = await bootSeat(
					fakeDaemon({
						pendingFyis: 0,
						send: () =>
							new Response(
								JSON.stringify({
									ok: false,
									command: "pij send",
									v: 2,
									error: "refused",
									meta: COLD_META,
								}),
								{ status: 400 },
							),
					}),
				);
				const refusal = await seat.send({ to: "pij-peer", message: "wake" }).then(
					() => undefined,
					(error: unknown) => (error instanceof Error ? error.message : String(error)),
				);
				expect(refusal).toBe(COLD_META);
			});

			it("renders the receipt's cold_check", async () => {
				seat = await bootSeat(
					fakeDaemon({
						pendingFyis: 0,
						send: (body) =>
							envelope("pij send", {
								msg_id: body.msg_id,
								outcome: "accepted",
								at: 1,
								cold_check: "unknown: no answer within 3s",
							}),
					}),
				);
				const result = (await seat.send({ to: "pij-peer", message: "wake" })) as {
					content: Array<{ text: string }>;
				};
				expect(result.content[0]?.text).toMatch(
					/ -> pij-peer \(cold-check: unknown: no answer within 3s\)$/,
				);
			});
		});

		it("renders the receipt's question warning on its own line", async () => {
			const warning = "this looks like a question; if you need an answer, resend without --fyi";
			seat = await bootSeat(
				fakeDaemon({
					pendingFyis: 0,
					send: (body) =>
						envelope("pij send", {
							msg_id: body.msg_id,
							outcome: { outcome: "held", reason: "fyi" },
							at: 1,
							warning,
						}),
				}),
			);
			const result = (await seat.send({
				to: "pij-peer",
				message: "can you check X?",
				fyi: true,
			})) as { content: Array<{ text: string }> };
			expect(result.content[0]?.text.split("\n")).toEqual([
				expect.stringMatching(/^held \(fyi\) \S+ -> pij-peer$/),
				warning,
			]);
		});
	});

	describe("before_agent_start", () => {
		it("returns the daemon's golden block verbatim as a displayed pij-fyi message", async () => {
			seat = await bootSeat(
				fakeDaemon({ pendingFyis: 2, claim: { count: 2, block: GOLDEN_BLOCK } }),
			);
			expect(await seat.beforeAgentStart()).toEqual({
				message: { customType: "pij-fyi", content: GOLDEN_BLOCK, display: true },
			});
		});

		it.each([
			["omp", "hook:omp", "1"],
			["pi", "hook:pi", undefined],
		] as const)("claims on %s as %s with the registered native session", async (_host, via, ompCode) => {
			vi.stubEnv("OMPCODE", ompCode);
			const daemon = fakeDaemon({ pendingFyis: 0, claim: { count: 0, block: "" } });
			seat = await bootSeat(daemon);
			await seat.beforeAgentStart();
			expect(pathed(daemon, "/v1/fyi/claim").map((request) => request.body)).toEqual([
				{ seat: daemon.state.seat, native_session: "native-fyi-session", via },
			]);
		});

		it.each([
			["nothing is pending", { count: 0, block: "" }],
			["the daemon refuses the claim", "refused"],
			["the daemon is down", "down"],
		] as const)("adds nothing when %s", async (_why, claim) => {
			seat = await bootSeat(fakeDaemon({ pendingFyis: 0, claim }));
			expect(await seat.beforeAgentStart()).toBeUndefined();
		});

		it("an OMP in-process subagent's turn claims nothing for the parent seat", async () => {
			vi.stubEnv("OMPCODE", "1");
			const daemon = fakeDaemon({ pendingFyis: 1, claim: { count: 1, block: GOLDEN_BLOCK } });
			seat = await bootSeat(daemon);
			expect(await seat.embeddedChildBeforeAgentStart()).toBeUndefined();
			expect(pathed(daemon, "/v1/fyi/claim")).toEqual([]);
		});
	});

	describe("✉N footer", () => {
		it("seeds ✉N from the state card when FYIs are pending", async () => {
			const current = await bootSeat(fakeDaemon({ pendingFyis: 2 }));
			seat = current;
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBe("✉2"));
		});

		it("never shows ✉0", async () => {
			const current = await bootSeat(fakeDaemon({ pendingFyis: 0 }));
			seat = current;
			// Boot clears the slot, then the seeded count 0 repaints it: two writes.
			await vi.waitFor(() => expect(current.fyiFooter()).toHaveLength(2));
			expect(current.fyiFooter()).toEqual([undefined, undefined]);
		});

		it("repaints on this seat's fyi events without injecting anything into the session", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			const current = await bootSeat(daemon);
			seat = current;
			const injectedAtBoot = current.injected.length;
			daemon.state.pendingFyis = 3;
			daemon.pushEvent("fyi.held", daemon.state.seat);
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBe("✉3"));
			daemon.state.pendingFyis = 0;
			daemon.pushEvent("seat.tombstone", daemon.state.seat);
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBeUndefined());
			expect(current.injected.length).toBe(injectedAtBoot);
		});

		it("ignores another seat's fyi events", async () => {
			const daemon = fakeDaemon({ pendingFyis: 1 });
			const current = await bootSeat(daemon);
			seat = current;
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBe("✉1"));
			const before = pathed(daemon, "/v1/state").length;
			daemon.state.pendingFyis = 5;
			daemon.pushEvent("fyi.held", "pij-someone-else");
			daemon.pushEvent("fyi.restored", daemon.state.seat);
			// Frames are handled in order: once ✉5 paints, both have been seen.
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBe("✉5"));
			expect(pathed(daemon, "/v1/state")).toHaveLength(before + 1);
		});

		it("clears after a successful claim empties the queue", async () => {
			const daemon = fakeDaemon({ pendingFyis: 2, claim: { count: 2, block: GOLDEN_BLOCK } });
			const current = await bootSeat(daemon);
			seat = current;
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBe("✉2"));
			daemon.state.pendingFyis = 0;
			await current.beforeAgentStart();
			await vi.waitFor(() => expect(current.fyiFooter().at(-1)).toBeUndefined());
		});
	});

	describe("turn activity", () => {
		it("publishes working then idle in order even when working resolves late", async () => {
			const gate = Promise.withResolvers<void>();
			const daemon = fakeDaemon({ pendingFyis: 0, firstActivityGate: gate.promise });
			seat = await bootSeat(daemon);
			await seat.turnStart();
			await seat.turnEnd();
			await vi.waitFor(() => expect(pathed(daemon, "/v1/activity")).toHaveLength(1));
			gate.resolve();
			await vi.waitFor(() => expect(daemon.applied).toEqual(["working", "idle"]));
			expect(pathed(daemon, "/v1/activity").map((request) => request.body)).toEqual([
				{ seat: daemon.state.seat, native_session: "native-fyi-session", state: "working" },
				{ seat: daemon.state.seat, native_session: "native-fyi-session", state: "idle" },
			]);
		});

		it("agent_end repairs idle when turn_end's idle publication failed", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			const current = await bootSeat(daemon);
			seat = current;
			await current.turnStart();
			await vi.waitFor(() => expect(daemon.applied).toEqual(["working"]));
			daemon.state.activity = "down";
			await current.turnEnd();
			await vi.waitFor(() => expect(pathed(daemon, "/v1/activity")).toHaveLength(2));
			daemon.state.activity = "up";
			await current.agentEnd(false);
			await vi.waitFor(() => expect(daemon.applied).toEqual(["working", "idle"]));
		});

		it("agent_end publishes nothing while the agent will continue", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			const current = await bootSeat(daemon);
			seat = current;
			await current.turnStart();
			await current.agentEnd(true);
			await current.turnStart();
			// Publications land in call order, so the second working proves agent_end was seen.
			await vi.waitFor(() => expect(daemon.applied).toHaveLength(2));
			expect(daemon.applied).toEqual(["working", "working"]);
		});

		it("stops publishing for the runtime once the daemon has no activity route", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			daemon.state.activity = "absent";
			const current = await bootSeat(daemon);
			await current.turnStart();
			await current.turnEnd();
			await current.turnStart();
			await current.agentEnd(false);
			// Shutdown settles every queued publication before it returns.
			await current.shutdown();
			expect(pathed(daemon, "/v1/activity")).toHaveLength(1);
		});
	});

	describe("session_shutdown", () => {
		it("publishes idle before returning when the seat was left working", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			const current = await bootSeat(daemon);
			await current.turnStart();
			await vi.waitFor(() => expect(daemon.applied).toEqual(["working"]));
			await current.shutdown();
			expect(daemon.applied).toEqual(["working", "idle"]);
		});

		it("publishes nothing more when the seat is already idle", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			const current = await bootSeat(daemon);
			await current.turnStart();
			await current.turnEnd();
			await vi.waitFor(() => expect(daemon.applied).toEqual(["working", "idle"]));
			await current.shutdown();
			expect(pathed(daemon, "/v1/activity")).toHaveLength(2);
		});

		it("completes without throwing when the daemon is down", async () => {
			const daemon = fakeDaemon({ pendingFyis: 0 });
			const current = await bootSeat(daemon);
			await current.turnStart();
			await vi.waitFor(() => expect(daemon.applied).toEqual(["working"]));
			daemon.state.activity = "down";
			await expect(current.shutdown()).resolves.toBeUndefined();
			expect(pathed(daemon, "/v1/activity").map((request) => request.body.state)).toEqual([
				"working",
				"idle",
			]);
		});
	});
});
