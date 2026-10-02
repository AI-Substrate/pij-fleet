import { describe, expect, it } from "vitest";

import {
	codexCwdFromMeta,
	codexRolloutForSession,
	codexSessionIdFromPath,
	codexTranscriptRoot,
	listCodexRollouts,
	summarizeCodexEvent,
} from "./codex.js";

// Fixtures lifted from a REAL rollout head (codex-cli 0.142.3, 2026-06-28):
//   ~/.codex/sessions/2026/06/28/rollout-2026-06-28T15-33-30-019f0cb7-…-0118.jsonl
const ROLLOUT =
	"/Users/jo/.codex/sessions/2026/06/28/" +
	"rollout-2026-06-28T15-33-30-019f0cb7-f65c-76f1-bb38-c96269590118.jsonl";
const UUID = "019f0cb7-f65c-76f1-bb38-c96269590118";

describe("codexTranscriptRoot", () => {
	it("is ~/.codex/sessions (global, NOT cwd-scoped like claude — F-04)", () => {
		expect(codexTranscriptRoot("/Users/jo")).toBe("/Users/jo/.codex/sessions");
	});
});

describe("codexSessionIdFromPath", () => {
	it("is the rollout filename's TRAILING UUID, not the stem (Finding 06)", () => {
		// The stem is `rollout-<ISO>-<uuid>`; the ISO segment also has hyphens/digits,
		// so the id must be the UUID-shaped suffix — never the whole stem.
		expect(codexSessionIdFromPath(ROLLOUT)).toBe(UUID);
	});

	it("matches the live convention: filename UUID == session_meta.id (POC)", () => {
		const p = `/x/y/rollout-2026-04-30T13-14-22-019ddc61-1234-7abc-8def-0123456789ab.jsonl`;
		expect(codexSessionIdFromPath(p)).toBe("019ddc61-1234-7abc-8def-0123456789ab");
	});
});

describe("codexRolloutForSession", () => {
	it("finds the rollout whose trailing UUID exactly matches an explicit native id", () => {
		const other = "/x/rollout-2026-06-28T00-00-00-aaaaaaaa-1111-7222-8333-bbbbbbbbbbbb.jsonl";
		expect(codexRolloutForSession([other, ROLLOUT], UUID)).toBe(ROLLOUT);
	});

	it("returns null when an authoritative id has no matching rollout", () => {
		expect(codexRolloutForSession([ROLLOUT], "aaaaaaaa-1111-7222-8333-bbbbbbbbbbbb")).toBeNull();
	});

	it("rejects an exact rollout path when the caller cannot read it", () => {
		expect(codexRolloutForSession([ROLLOUT], UUID, () => false)).toBeNull();
	});
});

describe("listCodexRollouts (recursive date-tree walk, F-03; R-4 multi-day)", () => {
	const A = "/r/2026/06/27/rollout-2026-06-27T09-00-00-aaaaaaaa-0001-7000-8000-000000000001.jsonl";
	const B = "/r/2026/06/28/rollout-2026-06-28T10-00-00-bbbbbbbb-0002-7000-8000-000000000002.jsonl";
	const tree: Record<string, string[]> = {
		"/r": ["2026", "README"],
		"/r/2026": ["06"],
		"/r/2026/06": ["27", "28"],
		"/r/2026/06/27": [A.slice(A.lastIndexOf("/") + 1)],
		"/r/2026/06/28": [B.slice(B.lastIndexOf("/") + 1), "notes.txt"],
		// "README" is a file → a real readdir on it returns [] (recursion no-ops).
	};
	const readDir = (d: string): string[] => tree[d] ?? [];

	it("collects rollout *.jsonl across BOTH date dirs (today + yesterday — R-4)", () => {
		expect(listCodexRollouts(readDir, "/r").sort()).toEqual([A, B].sort());
	});

	it("ignores non-rollout files and non-jsonl noise", () => {
		// notes.txt (not a rollout) is skipped; README (a file) recurses to [].
		expect(listCodexRollouts(readDir, "/r")).not.toContain("/r/2026/06/28/notes.txt");
	});

	it("returns [] for an unreadable / empty root (no codex sessions yet)", () => {
		expect(listCodexRollouts(() => [], "/missing")).toEqual([]);
	});
});

describe("summarizeCodexEvent (pij tail of a bound codex session, AC-04)", () => {
	it("renders a user turn from event_msg/user_message → [user] message", () => {
		const raw = JSON.stringify({
			timestamp: "2026-06-28T05:33:59Z",
			type: "event_msg",
			payload: {
				type: "user_message",
				message: "Reply with exactly this text and nothing else: CODEX_POC_OK_42",
			},
		});
		expect(summarizeCodexEvent(raw)).toEqual({
			role: "user",
			text: "Reply with exactly this text and nothing else: CODEX_POC_OK_42",
		});
	});

	it("renders an assistant turn from event_msg/agent_message → [assistant] message", () => {
		const raw = JSON.stringify({
			type: "event_msg",
			payload: { type: "agent_message", message: "CODEX_POC_OK_42", phase: "final_answer" },
		});
		expect(summarizeCodexEvent(raw)).toEqual({ role: "assistant", text: "CODEX_POC_OK_42" });
	});

	it("renders a tool call from response_item/function_call → assistant ⚙ name", () => {
		const raw = JSON.stringify({
			type: "response_item",
			payload: { type: "function_call", name: "shell", arguments: '{"command":["ls"]}' },
		});
		expect(summarizeCodexEvent(raw)).toEqual({ role: "assistant", text: "⚙ shell" });
	});

	it("drops developer/system response_item.message (permissions injection — noise)", () => {
		const raw = JSON.stringify({
			type: "response_item",
			payload: {
				type: "message",
				role: "developer",
				content: [{ type: "input_text", text: "<permissions instructions>…" }],
			},
		});
		expect(summarizeCodexEvent(raw)).toBeNull();
	});

	it("ignores non-conversational lines (session_meta/turn_context/reasoning/token_count)", () => {
		expect(
			summarizeCodexEvent(JSON.stringify({ type: "session_meta", payload: { id: UUID } })),
		).toBeNull();
		expect(summarizeCodexEvent(JSON.stringify({ type: "turn_context", payload: {} }))).toBeNull();
		expect(
			summarizeCodexEvent(
				JSON.stringify({ type: "response_item", payload: { type: "reasoning" } }),
			),
		).toBeNull();
		expect(
			summarizeCodexEvent(
				JSON.stringify({ type: "event_msg", payload: { type: "token_count", info: {} } }),
			),
		).toBeNull();
	});

	it("returns null for malformed JSON and empty messages", () => {
		expect(summarizeCodexEvent("{not json")).toBeNull();
		expect(
			summarizeCodexEvent(
				JSON.stringify({ type: "event_msg", payload: { type: "user_message", message: "" } }),
			),
		).toBeNull();
	});
});

describe("codexCwdFromMeta (cwd-confirm tiebreak for the global dir — R-2)", () => {
	it("extracts payload.cwd from the session_meta line (line 1)", () => {
		const raw = JSON.stringify({
			timestamp: "2026-06-28T05:33:59Z",
			type: "session_meta",
			payload: { id: UUID, cwd: "/Users/jordanknight/pi-hacking/pij", originator: "codex-tui" },
		});
		expect(codexCwdFromMeta(raw)).toBe("/Users/jordanknight/pi-hacking/pij");
	});

	it("returns null for a non-session_meta line or missing cwd or bad JSON", () => {
		expect(codexCwdFromMeta(JSON.stringify({ type: "event_msg", payload: {} }))).toBeNull();
		expect(codexCwdFromMeta(JSON.stringify({ type: "session_meta", payload: {} }))).toBeNull();
		expect(codexCwdFromMeta("not json")).toBeNull();
	});
});
