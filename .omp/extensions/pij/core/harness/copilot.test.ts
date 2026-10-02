import { describe, expect, it } from "vitest";

import {
	copilotStateRoot,
	resolveCopilotCurrentSession,
	sessionEventsPath,
	summarizeCopilotEvent,
} from "./copilot.js";

describe("sessionEventsPath", () => {
	it("is ~/.copilot/session-state/<sid>/events.jsonl", () => {
		expect(sessionEventsPath("/Users/jo", "9a8f8be6-3670-4e5c-b43e-09f46fe4dfad")).toBe(
			"/Users/jo/.copilot/session-state/9a8f8be6-3670-4e5c-b43e-09f46fe4dfad/events.jsonl",
		);
	});
});

describe("summarizeCopilotEvent", () => {
	// Shapes captured live from a real events.jsonl (Copilot v1.0.66, 2026-06-27).
	it("renders a user.message from data.content (not transformedContent)", () => {
		const raw = JSON.stringify({
			type: "user.message",
			data: {
				content: "say only the word PONG and nothing else",
				transformedContent: "<current_datetime>…</current_datetime>\n\nsay only the word PONG…",
			},
		});
		expect(summarizeCopilotEvent(raw)).toEqual({
			role: "user",
			text: "say only the word PONG and nothing else",
		});
	});

	it("renders an assistant.message from data.content", () => {
		const raw = JSON.stringify({
			type: "assistant.message",
			data: { messageId: "x", model: "gpt-5.5", content: "PONG", toolRequests: [] },
		});
		expect(summarizeCopilotEvent(raw)).toEqual({ role: "assistant", text: "PONG" });
	});

	it("appends tool requests as ⚙ name after assistant text", () => {
		const raw = JSON.stringify({
			type: "assistant.message",
			data: {
				content: "running it",
				toolRequests: [{ name: "bash" }, { name: "read" }],
			},
		});
		expect(summarizeCopilotEvent(raw)).toEqual({
			role: "assistant",
			text: "running it ⚙ bash ⚙ read",
		});
	});

	it("drops an empty-content user.message (deferred-tool/system injection)", () => {
		const raw = JSON.stringify({
			type: "user.message",
			data: { content: "", transformedContent: "IMPORTANT: deferred tools…" },
		});
		expect(summarizeCopilotEvent(raw)).toBeNull();
	});

	it("ignores non-conversational events (session/turn markers)", () => {
		expect(summarizeCopilotEvent(JSON.stringify({ type: "session.start", data: {} }))).toBeNull();
		expect(
			summarizeCopilotEvent(
				JSON.stringify({ type: "assistant.turn_start", data: { turnId: "0" } }),
			),
		).toBeNull();
	});

	it("returns null for malformed JSON", () => {
		expect(summarizeCopilotEvent("{not json")).toBeNull();
	});
});

// ─── Plan 040 F004: current Copilot identity resolution ─────────────────────

describe("copilotStateRoot", () => {
	it("is ~/.copilot/session-state", () => {
		expect(copilotStateRoot("/Users/jo")).toBe("/Users/jo/.copilot/session-state");
	});
});

describe("resolveCopilotCurrentSession", () => {
	const U1 = "9a8f8be6-3670-4e5c-b43e-09f46fe4dfad";
	const U2 = "11111111-2222-3333-4444-555555555555";

	it("uses COPILOT_AGENT_SESSION_ID even when another global session is newer", () => {
		const listState = (root: string) => {
			expect(root).toBe("/Users/jo/.copilot/session-state");
			return [
				{ name: U1, mtimeMs: 1000, isDirectory: true },
				{ name: U2, mtimeMs: 2000, isDirectory: true },
			];
		};
		expect(resolveCopilotCurrentSession(U1, listState, "/Users/jo")).toEqual({
			ok: true,
			sessionId: U1,
		});
	});

	it("rejects missing or invalid env ids without selecting a global directory", () => {
		const listState = () => [{ name: U2, mtimeMs: 9999, isDirectory: true }];
		expect(resolveCopilotCurrentSession(undefined, listState, "/Users/jo")).toMatchObject({
			ok: false,
			reason: "missing-env",
		});
		expect(resolveCopilotCurrentSession("not-a-uuid", listState, "/Users/jo")).toMatchObject({
			ok: false,
			reason: "invalid-env",
		});
	});

	it("requires matching session-state directory metadata for the env uuid", () => {
		expect(
			resolveCopilotCurrentSession(
				U1,
				() => [{ name: U2, mtimeMs: 9999, isDirectory: true }],
				"/Users/jo",
			),
		).toMatchObject({ ok: false, reason: "missing-state" });
		expect(
			resolveCopilotCurrentSession(
				U1,
				() => [{ name: U1, mtimeMs: 1, isDirectory: false }],
				"/Users/jo",
			),
		).toMatchObject({ ok: false, reason: "missing-state" });
	});
});
