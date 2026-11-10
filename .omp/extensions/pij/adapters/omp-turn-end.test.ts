import { describe, expect, it } from "vitest";

import { DaemonRefusalError } from "../core/daemon-wire.js";
import type { TurnEndResultWire } from "../core/turn-end.js";
import { registerOmpTurnEnd, type TurnEndRunner } from "./omp-turn-end.js";

type Handler = (event: unknown, ctx: unknown) => unknown;

/** The OMP surface registerOmpTurnEnd touches, recorded. */
function fakeOmp() {
	const handlers = new Map<string, Handler[]>();
	const sent: { customType: string; content: string; display: boolean }[] = [];
	const rerendered: number[] = [];
	let textRenderer: ((context: unknown) => string | undefined) | undefined;
	const api = {
		on(event: string, handler: Handler): void {
			handlers.set(event, [...(handlers.get(event) ?? []), handler]);
		},
		registerAssistantTextRenderer(renderer: (context: unknown) => string | undefined): void {
			textRenderer = renderer;
		},
		registerMessageRenderer(): void {},
		sendMessage(message: { customType: string; content: string; display: boolean }): void {
			sent.push(message);
		},
	};
	const ctx = {
		ui: {
			notify(): void {},
			rerenderMessage(timestamp: number): void {
				rerendered.push(timestamp);
			},
		},
		isIdle: () => true,
	};
	const emit = async (event: string, payload: unknown): Promise<unknown[]> =>
		Promise.all((handlers.get(event) ?? []).map((handler) => handler(payload, ctx)));
	return { api, sent, rerendered, emit, render: (context: unknown) => textRenderer?.(context) };
}

function runner(answer: TurnEndResultWire | Error): TurnEndRunner & { calls: string[] } {
	const calls: string[] = [];
	return {
		calls,
		async turnEnd(text: string): Promise<TurnEndResultWire> {
			calls.push(text);
			if (answer instanceof Error) throw answer;
			return answer;
		},
	};
}

const BLOCK = 'Done.\n\n```pij\nsend pij-a "one"\n```';
const RAN: TurnEndResultWire = {
	outcome: "ran",
	lines: [],
	ran: 1,
	summary: "pij ✓ 1 sent",
	followup: null,
};
const REFUSED: TurnEndResultWire = {
	outcome: "refused",
	lines: [],
	ran: 0,
	summary: "pij ✗ line 1 refused: E-RS-COLD-WAKE (0 ran, 0 not run)",
	followup: "[pij] Your end-of-turn pij block stopped at a refusal.",
};

function stop(text: string, stopHookActive = false, stopReason = "stop") {
	return {
		last_assistant_message: { role: "assistant", stopReason, content: [{ type: "text", text }] },
		stop_hook_active: stopHookActive,
	};
}

describe("registerOmpTurnEnd", () => {
	it("hands a reply that ends with a block to the daemon once, then posts its line when idle", async () => {
		const omp = fakeOmp();
		const daemon = runner(RAN);
		registerOmpTurnEnd(omp.api, () => daemon);
		expect(await omp.emit("session_stop", stop(BLOCK))).toEqual([undefined]);
		expect(daemon.calls).toEqual([BLOCK]);
		await omp.emit("agent_end", { willContinue: false });
		expect(omp.sent).toEqual([
			{ customType: "pij-turn-end", content: "pij ✓ 1 sent", display: true },
		]);
	});

	it("makes no call for a reply without a block, or one that did not stop cleanly", async () => {
		const omp = fakeOmp();
		const daemon = runner(RAN);
		registerOmpTurnEnd(omp.api, () => daemon);
		await omp.emit("session_stop", stop("just prose"));
		await omp.emit("session_stop", stop(BLOCK, false, "error"));
		expect(daemon.calls).toEqual([]);
	});

	it("blocks with the follow-up on failure, but never while the stop hook is active", async () => {
		const omp = fakeOmp();
		registerOmpTurnEnd(omp.api, () => runner(REFUSED));
		expect(await omp.emit("session_stop", stop(BLOCK, false))).toEqual([
			{ decision: "block", reason: REFUSED.followup },
		]);
		expect(await omp.emit("session_stop", stop(BLOCK, true))).toEqual([undefined]);
	});

	it("tells the seat to check before resending when the outcome is unknown", async () => {
		const omp = fakeOmp();
		registerOmpTurnEnd(omp.api, () => runner(new Error("timeout")));
		const [answer] = (await omp.emit("session_stop", stop(BLOCK))) as [{ reason: string }];
		expect(answer.reason).toContain("may have run");
	});

	it("says nothing ran when the daemon refused the call", async () => {
		const omp = fakeOmp();
		const refusal = new DaemonRefusalError("pij turn-end", "refused", "E-RS-ARG: bad body");
		registerOmpTurnEnd(omp.api, () => runner(refusal));
		const [answer] = (await omp.emit("session_stop", stop(BLOCK))) as [{ reason: string }];
		expect(answer.reason).toContain("Nothing was sent.");
	});

	it("hides the block on screen and redraws a reply that ended in a tool call as written", async () => {
		const omp = fakeOmp();
		registerOmpTurnEnd(omp.api, () => runner(RAN));
		const message = { timestamp: 3 };
		expect(omp.render({ message, text: BLOCK, streaming: false })).toBe(
			"Done.\n\n⚙ pij: 1 command(s) handed to pij",
		);
		await omp.emit("message_end", {
			message: {
				role: "assistant",
				stopReason: "toolUse",
				timestamp: 3,
				content: [{ type: "text", text: BLOCK }],
			},
		});
		expect(omp.rerendered).toEqual([3]);
		expect(omp.render({ message, text: BLOCK, streaming: false })).toBeUndefined();
	});
});
