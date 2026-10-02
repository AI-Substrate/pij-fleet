// s099 — the pij_send registration schema must express message XOR command
// STRUCTURALLY, not only in the runtime guard (pij#166).
//
// Own file, not index.test.ts: that spec is a shared surface (fleet partition
// category 4) and this stream owns only the pij_send registration schema.
//
// ONE CRITERION, ONE CLAIM, ONE OBSERVABLE. Each behavioural test carries a
// single claim-bearing assertion and NO preceding setup assertion — a pre-fix
// red proves only the first assertion that fired, and a setup assertion in
// front of the claim promotes a precondition to evidence.
//
// Deliberately NOT a whole-object toEqual over the schema: that is one fat
// assertion proving "the object differs" without identifying which field —
// evidence narrowed by conflating rather than by aborting.
//
// SCOPE LIMIT, stated because it is the whole of pij#166: these assertions
// observe the REGISTRATION BOUNDARY. They cannot prove what the model is
// shown. See docs/plans/099-send-tool-xor/assets/union-spike.md — C1 is
// discharged only by observing a live seat's rendered tool definition.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Value } from "typebox/value";
import { describe, expect, it } from "vitest";

import activate from "./index.js";

/** Records tool definitions instead of discarding them. */
type Captured = {
	parameters: unknown;
	execute: (...args: unknown[]) => unknown;
};

function captureTools(): Map<string, Captured> {
	const tools = new Map<string, Captured>();
	const pi = {
		on: () => {},
		registerTool: (tool: { name: string; parameters: unknown; execute: Captured["execute"] }) => {
			tools.set(tool.name, { parameters: tool.parameters, execute: tool.execute });
		},
		registerCommand: () => {},
		events: { on: () => {}, emit: () => {} },
		sendUserMessage: () => {},
		sendMessage: () => {},
		setSessionName: async () => {},
	} as unknown as ExtensionAPI;
	activate(pi);
	return tools;
}

function sendTool(): Captured {
	const tool = captureTools().get("pij_send");
	if (!tool) throw new Error("pij_send was not registered — the probe is broken, not the schema");
	return tool;
}

function sendSchema(): Record<string, unknown> {
	return sendTool().parameters as Record<string, unknown>;
}

/** Invoke the registered execute() with only the params that reach the XOR
 *  guard. The guard is the first statement, before any dependency is touched,
 *  so no registry/ctx is required to observe it. */
function callSend(params: Record<string, unknown>): Promise<unknown> {
	return Promise.resolve(
		sendTool().execute("call-1", params, undefined, undefined, { cwd: process.cwd() }),
	);
}

describe("pij_send registration schema — structural XOR (pij#166)", () => {
	// --- CONTROL -----------------------------------------------------------
	// Not a criterion. Proves the probe reaches a real schema, so that the
	// negative results below mean something. A zero from a broken probe and a
	// zero from a correct one are the same observable.
	it("CONTROL: the probe captures a pij_send schema with a `to` property", () => {
		const props = sendSchema().properties as Record<string, unknown> | undefined;
		expect(props && Object.keys(props)).toContain("to");
	});

	// --- C1 · BEHAVIOURAL --------------------------------------------------
	it("C1: the schema carries a structural exclusivity constraint", () => {
		const s = sendSchema();
		expect(s.oneOf ?? s.anyOf ?? s.not).toBeDefined();
	});

	// --- C2 · BEHAVIOURAL --------------------------------------------------
	it("C2: {to, message, command} is NOT schema-valid", () => {
		expect(
			Value.Check(sendSchema() as never, { to: "pij-x", message: "hi", command: "compact" }),
		).toBe(false);
	});

	// --- C3 · BEHAVIOURAL --------------------------------------------------
	it("C3: {to} alone is NOT schema-valid", () => {
		expect(Value.Check(sendSchema() as never, { to: "pij-x" })).toBe(false);
	});

	// --- C4 · PRESERVED-PROPERTY -------------------------------------------
	// True before AND after. A regression guard, NEVER evidence the fix works.
	it("C4 [preserved]: {to, message} is schema-valid", () => {
		expect(Value.Check(sendSchema() as never, { to: "pij-x", message: "hi" })).toBe(true);
	});

	// --- C5 · PRESERVED-PROPERTY -------------------------------------------
	it("C5 [preserved]: {to, command} is schema-valid", () => {
		expect(Value.Check(sendSchema() as never, { to: "pij-x", command: "compact" })).toBe(true);
	});

	// --- C6a / C6b · PRESERVED-PROPERTY ------------------------------------
	// The runtime XOR guard in execute() must keep rejecting both invalid
	// shapes. True before AND after this stream's schema change — a regression
	// guard, never evidence the fix works.
	//
	// These exist because the guard throwing "needs exactly one of" in pij_send's
	// execute() had NO test anywhere in the repo, despite being the sole
	// enforcement of the rule for the whole of pij#166. Striking C6 as "covered
	// elsewhere" would have been false.
	//
	// Cited by the string it throws, not by a line number: a citation keyed on
	// something that moves goes stale silently, and a formatter moved this one
	// while the file was being written.

	it("C6a [preserved]: execute() rejects {to} at runtime", async () => {
		await expect(callSend({ to: "pij-x" })).rejects.toThrow("exactly one of");
	});

	it("C6b [preserved]: execute() rejects {to, message, command} at runtime", async () => {
		await expect(callSend({ to: "pij-x", message: "hi", command: "compact" })).rejects.toThrow(
			"exactly one of",
		);
	});

	// --- Plan 158 · fyi is message-only ------------------------------------
	it("C7: {to, command, fyi: true} is NOT schema-valid", () => {
		expect(Value.Check(sendSchema() as never, { to: "pij-x", command: "compact", fyi: true })).toBe(
			false,
		);
	});

	it("C8: {to, message, fyi: true} is schema-valid", () => {
		expect(Value.Check(sendSchema() as never, { to: "pij-x", message: "hi", fyi: true })).toBe(
			true,
		);
	});

	// --- Plan 157 phase 2 · force needs a reason, and only wakes a message ---
	it("C9: {to, message, force: true, reason} is schema-valid", () => {
		expect(
			Value.Check(sendSchema() as never, {
				to: "pij-x",
				message: "hi",
				force: true,
				reason: "the review blocks the release",
			}),
		).toBe(true);
	});

	it("C10: {to, message, force: true} without a reason is NOT schema-valid", () => {
		expect(Value.Check(sendSchema() as never, { to: "pij-x", message: "hi", force: true })).toBe(
			false,
		);
	});

	it("C11: a blank reason does not satisfy force", () => {
		expect(
			Value.Check(sendSchema() as never, { to: "pij-x", message: "hi", force: true, reason: "  " }),
		).toBe(false);
	});

	it("C12: force with fyi is NOT schema-valid", () => {
		expect(
			Value.Check(sendSchema() as never, {
				to: "pij-x",
				message: "hi",
				fyi: true,
				force: true,
				reason: "why",
			}),
		).toBe(false);
	});

	it("C13: force with a control command is NOT schema-valid", () => {
		expect(
			Value.Check(sendSchema() as never, {
				to: "pij-x",
				command: "compact",
				force: true,
				reason: "why",
			}),
		).toBe(false);
	});
});
