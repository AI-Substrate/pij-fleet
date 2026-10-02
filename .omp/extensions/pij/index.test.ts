// pij index wiring — status bar (plan 018).
//
// Pattern P8: tests target the wiring, not the core. This file owns ONE
// concern: session_start publishes the pij id through every applicable persistent
// status surface — Pi's keyed extension status and OMP's default session_name
// segment.
//
// Non-vacuous: removing either publication path from index.ts fails its
// corresponding assertion.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as daemonHttp from "./adapters/daemon-http.js";
import { PiRuntimeAdapter } from "./adapters/pi-runtime.js";
import pijExtension from "./index.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Minimal fake ExtensionAPI — captures only what index.ts needs at wiring
 *  time + the runtime-specific message API used for the boot announce. */
function makeFakePi(
	onSendMessage: (message: string) => void = () => {},
	sessionNameTakesAfter = 1,
) {
	const handlers = new Map<string, (...args: unknown[]) => unknown>();
	const sentMessages: string[] = [];
	const sessionNames: string[] = [];
	const tools = new Map<string, { execute: (...args: unknown[]) => unknown }>();
	let currentSessionName: string | undefined;
	const pi = {
		on: (event: string, handler: (...args: unknown[]) => unknown) => {
			handlers.set(event, handler);
		},
		registerTool: (tool: { name: string; execute: (...args: unknown[]) => unknown }) => {
			tools.set(tool.name, tool);
		},
		registerCommand: () => {},
		events: { on: () => {}, emit: () => {} },
		sendUserMessage: (message: string) => {
			sentMessages.push(message);
			onSendMessage(message);
		},
		sendMessage: (message: { content: string }) => {
			sentMessages.push(message.content);
			onSendMessage(message.content);
		},
		setSessionName: async (name: string) => {
			sessionNames.push(name);
			if (sessionNames.length >= sessionNameTakesAfter) currentSessionName = name;
		},
		getSessionName: () => currentSessionName,
	} as unknown as ExtensionAPI;
	return { pi, handlers, sentMessages, sessionNames, tools };
}

/** Minimal fake ExtensionContext — captures setStatus calls. */
function makeFakeCtx(
	initialSessionId: string | undefined = "test-session-statusbar-018",
	options: { mode?: string; entries?: readonly { type: string }[] } = {},
) {
	const statuses: Array<{ key: string; value: string | undefined }> = [];
	const notices: string[] = [];
	let sessionId = initialSessionId;
	const ctx = {
		hasUI: true,
		mode: options.mode,
		sessionManager: {
			getSessionId: () => sessionId,
			getEntries: () => options.entries ?? [],
		},
		isIdle: () => true,
		compact: () => {},
		ui: {
			setStatus: (key: string, value: string | undefined) => statuses.push({ key, value }),
			notify: (message: string) => notices.push(message),
		},
	} as unknown as ExtensionContext;
	return { ctx, statuses, notices, setSessionId: (next: string | undefined) => (sessionId = next) };
}

function rustEnvelope(command: string, data: unknown): Response {
	return new Response(JSON.stringify({ ok: true, command, v: 2, data }));
}

function fakeRustGeneration(claims: Record<string, unknown>[] = []): daemonHttp.DaemonGeneration {
	const fetchMock = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
		const path = new URL(String(url)).pathname;
		if (path === "/v1/inbox") {
			return rustEnvelope("pij inbox", []);
		}
		if (path === "/v1/seats") {
			return rustEnvelope("pij seats", { seats: [], unavailable: [] });
		}
		if (path === "/v1/register") {
			const claim = JSON.parse(String(init?.body)) as Record<string, unknown>;
			claims.push(claim);
			return rustEnvelope("pij register", {
				id: claim.id,
				harness: claim.harness,
				proc: { pid: claim.pid, proc_start: claim.proc_start },
				folder: claim.folder,
				state: "idle",
				parent: claim.parent ?? null,
			});
		}
		if (path === "/v1/events") {
			return new Response(`${JSON.stringify({ hello: true, v: 1, build: "pij-rs test" })}\n`);
		}
		throw new Error(`unexpected Rust fixture path ${path}`);
	});
	const location = { addr: "127.0.0.1:7461", stateDir: "/unused" };
	return {
		kind: "rust",
		location,
		health: { status: "healthy", build: "pij-rs test", offline: true, machine: "test" },
		client: new daemonHttp.PijDaemonClient(location, "secret", {
			fetch: fetchMock as typeof fetch,
			readFile: async () => "secret",
			processStart: () => 20260831134200,
		}),
	};
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("pij index — footer status bar", () => {
	let pijHome: string;
	let origPijHome: string | undefined;
	let origSessionId: string | undefined;
	let origParentId: string | undefined;
	let origPlanId: string | undefined;
	let origOmpCode: string | undefined;
	let origAnnounceTo: string | undefined;
	let origDaemonGeneration: string | undefined;

	beforeEach(() => {
		origPijHome = process.env.PIJ_HOME;
		origSessionId = process.env.PIJ_SESSION_ID;
		origParentId = process.env.PIJ_PARENT_ID;
		origPlanId = process.env.PIJ_PLAN_ID;
		origOmpCode = process.env.OMPCODE;
		origAnnounceTo = process.env.PIJ_ANNOUNCE_TO;
		origDaemonGeneration = process.env.PIJ_DAEMON_GENERATION;
		pijHome = mkdtempSync(join(tmpdir(), "pij-status-test-"));
		process.env.PIJ_HOME = pijHome;
		// Every test runs against a faked rs daemon, never a live one on this machine.
		delete process.env.PIJ_DAEMON_GENERATION;
		vi.spyOn(daemonHttp, "detectDaemonGeneration").mockResolvedValue(fakeRustGeneration());
		delete process.env.PIJ_PARENT_ID;
		delete process.env.PIJ_PLAN_ID;
		delete process.env.OMPCODE;
		delete process.env.PIJ_ANNOUNCE_TO;
	});

	afterEach(() => {
		vi.restoreAllMocks();
		rmSync(pijHome, { recursive: true, force: true });
		if (origPijHome === undefined) delete process.env.PIJ_HOME;
		else process.env.PIJ_HOME = origPijHome;
		if (origSessionId === undefined) delete process.env.PIJ_SESSION_ID;
		else process.env.PIJ_SESSION_ID = origSessionId;
		if (origParentId === undefined) delete process.env.PIJ_PARENT_ID;
		else process.env.PIJ_PARENT_ID = origParentId;
		if (origPlanId === undefined) delete process.env.PIJ_PLAN_ID;
		else process.env.PIJ_PLAN_ID = origPlanId;
		if (origOmpCode === undefined) delete process.env.OMPCODE;
		else process.env.OMPCODE = origOmpCode;
		if (origAnnounceTo === undefined) delete process.env.PIJ_ANNOUNCE_TO;
		else process.env.PIJ_ANNOUNCE_TO = origAnnounceTo;
		if (origDaemonGeneration === undefined) delete process.env.PIJ_DAEMON_GENERATION;
		else process.env.PIJ_DAEMON_GENERATION = origDaemonGeneration;
	});

	it.each([
		"unchanged",
		"replaced",
		"cleared",
	] as const)("preserves a resend draft while respecting %s human input", async (action) => {
		process.env.OMPCODE = "1";
		vi.useFakeTimers();
		try {
			const { pi, handlers } = makeFakePi();
			const { ctx } = makeFakeCtx();
			let editorText = "HUMAN DRAFT IN PROGRESS do not clobber me";
			const terminalListeners = new Set<(data: string) => unknown>();
			ctx.ui.getEditorText = () => editorText;
			ctx.ui.setEditorText = (text) => {
				editorText = text;
			};
			ctx.ui.onTerminalInput = (listener) => {
				terminalListeners.add(listener);
				return () => terminalListeners.delete(listener);
			};
			const queued: string[] = [];
			pi.sendUserMessage = (content) => {
				queued.push(String(content));
			};
			pijExtension(pi);
			const runtime = new PiRuntimeAdapter(pi, ctx, "omp");
			runtime.inject("Incoming peer body", "steer", "resend-own-id", 1);
			// A draft never delays the prompt; typing during its queue wait
			// must survive, rather than restoring the stale injection snapshot.
			expect(queued).toHaveLength(1);
			editorText += "\nMore typing 你好";
			const draftAtConsumption = editorText;
			const prompt = queued.shift();
			if (prompt === undefined) throw new Error("resend was not submitted immediately");
			await handlers.get("message_start")?.(
				{ message: { role: "user", content: [{ type: "text", text: prompt }] } },
				ctx,
			);
			// OMP awaits extension handlers, THEN its EventController clears.
			ctx.ui.setEditorText("");
			if (action === "replaced") editorText = "New human draft";
			if (action === "cleared") {
				for (const listener of terminalListeners) listener("human input");
				editorText = "";
			}
			await vi.runAllTimersAsync();
			expect(editorText).toBe(
				action === "unchanged"
					? draftAtConsumption
					: action === "replaced"
						? "New human draft"
						: "",
			);
			expect(queued).toEqual([]);
			expect(terminalListeners.size).toBe(0);
		} finally {
			vi.useRealTimers();
		}
	});

	it("keeps an OMP parent's registration when in-process children start and stop", async () => {
		process.env.OMPCODE = "1";
		const detect = vi
			.spyOn(daemonHttp, "detectDaemonGeneration")
			.mockResolvedValue(fakeRustGeneration());
		const parent = makeFakePi();
		const parentContext = makeFakeCtx("native-omp-parent", { mode: "tui" });
		pijExtension(parent.pi);
		await parent.handlers.get("session_start")?.({ reason: "startup" }, parentContext.ctx);
		const parentId = process.env.PIJ_SESSION_ID;
		try {
			for (const childId of ["native-omp-child-one", "native-omp-child-two"]) {
				const child = makeFakePi();
				const childContext = makeFakeCtx(childId, {
					mode: "print",
					entries: [{ type: "session_init" }],
				});
				childContext.ctx.hasUI = false;
				pijExtension(child.pi);
				await child.handlers.get("session_start")?.({ reason: "startup" }, childContext.ctx);
				await child.handlers.get("session_shutdown")?.({ reason: "quit" }, childContext.ctx);
				expect(child.sentMessages).toEqual([]);
				expect(child.sessionNames).toEqual([]);
				expect(process.env.PIJ_SESSION_ID).toBe(parentId);
				for (const [name, params] of [
					["pij_send", { to: "peer", message: "hello" }],
					["pij_spawn", { harness: "omp", task: "hello" }],
					["pij_close", { to: "peer" }],
				] as const) {
					const tool = child.tools.get(name);
					if (!tool) throw new Error(`Child tool probe did not capture ${name}`);
					await expect(
						tool.execute("probe", params, undefined, undefined, childContext.ctx),
					).rejects.toThrow("OMP in-process subagent");
				}
			}
			expect(detect).toHaveBeenCalledTimes(1);
		} finally {
			await parent.handlers.get("session_shutdown")?.({ reason: "quit" }, parentContext.ctx);
		}
	});

	it.each([
		"print",
		"rpc",
	])("preserves an independently hosted headless OMP %s root", async (mode) => {
		process.env.OMPCODE = "1";
		const detect = vi
			.spyOn(daemonHttp, "detectDaemonGeneration")
			.mockResolvedValue(fakeRustGeneration());
		const { pi, handlers, sentMessages } = makeFakePi();
		const { ctx } = makeFakeCtx("native-headless-omp", { mode });
		ctx.hasUI = false;
		pijExtension(pi);
		try {
			await handlers.get("session_start")?.({ reason: "startup" }, ctx);
			expect(detect).toHaveBeenCalledOnce();
			expect(sentMessages).toHaveLength(1);
		} finally {
			await handlers.get("session_shutdown")?.({ reason: "quit" }, ctx);
		}
	});

	it("retains headless Pi registration with session metadata", async () => {
		const detect = vi
			.spyOn(daemonHttp, "detectDaemonGeneration")
			.mockResolvedValue(fakeRustGeneration());
		const { pi, handlers, sentMessages } = makeFakePi();
		const { ctx } = makeFakeCtx("native-headless-pi", {
			mode: "print",
			entries: [{ type: "session_init" }],
		});
		ctx.hasUI = false;
		pijExtension(pi);
		try {
			await handlers.get("session_start")?.({ reason: "startup" }, ctx);
			expect(detect).toHaveBeenCalledOnce();
			expect(sentMessages).toHaveLength(1);
		} finally {
			await handlers.get("session_shutdown")?.({ reason: "quit" }, ctx);
		}
	});

	it("marks the selected default Rust generation in Pi's status segment", async () => {
		// daemon-http.test.ts owns the default-selection policy; this assertion owns
		// index.ts's UI wiring after that policy selects Rust.
		delete process.env.PIJ_DAEMON_GENERATION;
		vi.spyOn(daemonHttp, "detectDaemonGeneration").mockResolvedValue(fakeRustGeneration());
		const { pi, handlers } = makeFakePi();
		const { ctx, statuses, notices } = makeFakeCtx("native-rust-statusbar");

		pijExtension(pi);
		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);

		expect(statuses.find((status) => status.key === "pij")?.value).toMatch(
			/^pij-[a-z]+(-[a-z]+)* · rs-v1 · ext (?:[0-9a-f]{10}(?:\+dirty)?|hash:[0-9a-f]{12})$/,
		);
		expect(notices).toContainEqual(expect.stringContaining("detected rs daemon at"));
		expect(notices.every((notice) => !notice.includes("Rust daemon v1"))).toBe(true);
		await handlers.get("session_shutdown")?.({}, ctx);
	});

	it("publishes the selected default Rust id through OMP's session_name segment", async () => {
		delete process.env.PIJ_DAEMON_GENERATION;
		process.env.OMPCODE = "1";
		vi.spyOn(daemonHttp, "detectDaemonGeneration").mockResolvedValue(fakeRustGeneration());
		const { pi, handlers, sessionNames } = makeFakePi();
		const { ctx, statuses } = makeFakeCtx("native-rust-omp-statusbar");

		pijExtension(pi);
		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);

		const sessionName = sessionNames.at(-1);
		expect(sessionName).toMatch(
			/^rs·pij-[a-z]+(-[a-z]+)* · ext (?:[0-9a-f]{10}(?:\+dirty)?|hash:[0-9a-f]{12})$/,
		);
		expect(statuses.find((status) => status.key === "pij")?.value).toBe(
			`\u001b[33mrs\u001b[0m ${sessionName?.slice(3)}`,
		);
		await handlers.get("session_shutdown")?.({}, ctx);
	});

	it("does not reassert after startup publication has settled successfully", async () => {
		process.env.OMPCODE = "1";
		const { pi, handlers, sessionNames } = makeFakePi();
		const { ctx } = makeFakeCtx("omp-first-turn-already-took");

		pijExtension(pi);
		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(sessionNames).toHaveLength(1);

		handlers.get("turn_start")?.({ timestamp: Date.now() });
		expect(sessionNames).toHaveLength(1);
		await handlers.get("session_shutdown")?.({ reason: "quit" }, ctx);
	});

	it("reasserts immediately on the first turn when startup publication missed", async () => {
		process.env.OMPCODE = "1";
		const { pi, handlers, sessionNames } = makeFakePi(() => {}, 2);
		const { ctx, notices } = makeFakeCtx("omp-first-turn-reassert");

		pijExtension(pi);
		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
		expect(sessionNames).toHaveLength(1);

		handlers.get("turn_start")?.({ timestamp: Date.now() });
		expect(sessionNames).toHaveLength(2);
		expect(sessionNames.at(-1)).toMatch(/^rs·pij-/);

		await new Promise((resolve) => setTimeout(resolve, 300));
		expect(notices.filter((message) => message.includes("omp session name"))).toEqual([]);
		await handlers.get("session_shutdown")?.({ reason: "quit" }, ctx);
	});

	it("announces recovery when the first turn succeeds after the retry notice", async () => {
		vi.useFakeTimers();
		try {
			delete process.env.PIJ_DAEMON_GENERATION;
			process.env.OMPCODE = "1";
			vi.spyOn(daemonHttp, "detectDaemonGeneration").mockResolvedValue(fakeRustGeneration());
			const { pi, handlers, sessionNames } = makeFakePi(() => {}, 6);
			const { ctx, notices } = makeFakeCtx("omp-late-turn-recovery");

			pijExtension(pi);
			await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
			await vi.advanceTimersByTimeAsync(10_000);
			expect(sessionNames).toHaveLength(5);
			expect(notices.filter((message) => message.includes("omp session name"))).toEqual([
				expect.stringContaining("could not set omp session name"),
			]);

			handlers.get("turn_start")?.({ timestamp: Date.now() });
			await vi.advanceTimersByTimeAsync(0);
			expect(sessionNames).toHaveLength(6);
			expect(notices.filter((message) => message.includes("omp session name"))).toEqual([
				expect.stringContaining("could not set omp session name"),
				expect.stringContaining("omp session name set after retry"),
			]);
			await handlers.get("session_shutdown")?.({ reason: "quit" }, ctx);
		} finally {
			vi.useRealTimers();
		}
	});

	it("cancels a stale retry window on session shutdown", async () => {
		process.env.OMPCODE = "1";
		const { pi, handlers, sessionNames } = makeFakePi(() => {}, Number.POSITIVE_INFINITY);
		const { ctx } = makeFakeCtx("omp-shutdown-cancel");

		pijExtension(pi);
		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
		expect(sessionNames).toHaveLength(1);

		await handlers.get("session_shutdown")?.({ reason: "quit" }, ctx);
		await new Promise((resolve) => setTimeout(resolve, 300));
		expect(sessionNames).toHaveLength(1);
	});

	it("registers with its structural parent from PIJ_PARENT_ID", async () => {
		process.env.PIJ_PARENT_ID = "pij-structural-parent";
		const claims: Record<string, unknown>[] = [];
		vi.spyOn(daemonHttp, "detectDaemonGeneration").mockResolvedValue(fakeRustGeneration(claims));
		const { pi, handlers } = makeFakePi();
		const { ctx } = makeFakeCtx("native-repository");
		pijExtension(pi);

		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);

		expect(claims.at(-1)).toMatchObject({ parent: "pij-structural-parent", folder: process.cwd() });
		await handlers.get("session_shutdown")?.({}, ctx);
	});

	it("blocks ask_user_question only for a structurally managed Pi peer", async () => {
		process.env.PIJ_PARENT_ID = "pij-structural-parent";
		const { pi, handlers } = makeFakePi();
		const { ctx } = makeFakeCtx("native-modal-guard");
		pijExtension(pi);

		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
		const result = await handlers.get("tool_call")?.(
			{ toolCallId: "modal-call", toolName: "ask_user_question", input: {} },
			ctx,
		);

		expect(result).toMatchObject({
			block: true,
			reason: expect.stringContaining("pij invariant #9"),
		});
		expect(result).toMatchObject({
			reason: expect.stringContaining("pij_send"),
		});

		await handlers.get("session_shutdown")?.({}, ctx);
	});

	it("does not block ask_user_question for an un-managed Pi session", async () => {
		const { pi, handlers } = makeFakePi();
		const { ctx } = makeFakeCtx("native-generic-modal-guard");
		pijExtension(pi);

		await handlers.get("session_start")?.({ type: "session_start", reason: "startup" }, ctx);
		expect(
			await handlers.get("tool_call")?.(
				{ toolCallId: "generic-modal-call", toolName: "ask_user_question", input: {} },
				ctx,
			),
		).toBeUndefined();

		await handlers.get("session_shutdown")?.({}, ctx);
	});
});
