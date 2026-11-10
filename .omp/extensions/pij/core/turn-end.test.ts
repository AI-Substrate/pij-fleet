import { describe, expect, it } from "vitest";

import {
	decodeTurnEndResult,
	hideTrailingBlock,
	mayEndWithBlock,
	noResultDecision,
	stopDecision,
	TurnEndDisplay,
	type TurnEndResultWire,
	textOf,
} from "./turn-end.js";

const FENCE = "```";
const BLOCK = `Done.\n\n${FENCE}pij\nsend pij-a "one"\nsend pij-b "two"\n${FENCE}\n`;

function result(overrides: Partial<TurnEndResultWire>): TurnEndResultWire {
	return {
		outcome: "ran",
		lines: [],
		ran: 2,
		summary: "pij ✓ 2 sent",
		followup: null,
		...overrides,
	};
}

describe("mayEndWithBlock", () => {
	it("is true only when a pij fence closes the text", () => {
		expect(mayEndWithBlock(BLOCK)).toBe(true);
		expect(mayEndWithBlock(`a\n${FENCE}js\nx\n${FENCE}`)).toBe(false);
		expect(mayEndWithBlock(`${FENCE}pij\nsend a b\n${FENCE}\nmore prose`)).toBe(false);
		expect(mayEndWithBlock("no block")).toBe(false);
	});
});

describe("textOf", () => {
	it("joins the text blocks and skips the rest", () => {
		expect(
			textOf({
				content: [
					{ type: "thinking", thinking: "x" },
					{ type: "text", text: "a" },
					{ type: "text", text: "b" },
				],
			}),
		).toBe("a\nb");
		expect(textOf({ content: "plain" })).toBe("plain");
		expect(textOf(undefined)).toBe("");
	});
});

describe("hideTrailingBlock", () => {
	it("replaces the closing block with one line", () => {
		expect(hideTrailingBlock(BLOCK, false)).toEqual({
			hidden: true,
			shown: "Done.\n\n⚙ pij: 2 command(s) handed to pij",
			commands: 2,
		});
	});

	it("hides the block mid-stream, counting as it grows", () => {
		const partial = `Done.\n\n${FENCE}pij\nsend pij-a "one"\nsend pij-b "tw`;
		expect(hideTrailingBlock(partial, true)).toEqual({
			hidden: true,
			shown: "Done.\n\n⚙ pij: 2 command(s)…",
			commands: 2,
		});
		const opening = `Done.\n${FENCE}pi`;
		expect(hideTrailingBlock(opening, true)).toEqual({ hidden: true, shown: "Done.", commands: 0 });
		// Every prefix of a streamed reply hides the block: no raw fence ever draws.
		for (let i = BLOCK.indexOf(FENCE) + 1; i <= BLOCK.length; i++) {
			const view = hideTrailingBlock(BLOCK.slice(0, i), true);
			expect(view.hidden).toBe(true);
			if (view.hidden) expect(view.shown.includes("`")).toBe(false);
		}
	});

	it("draws a block that does not end the reply as written", () => {
		expect(hideTrailingBlock(`${FENCE}pij\nsend a b\n${FENCE}\nprose after`, false)).toEqual({
			hidden: false,
		});
		expect(hideTrailingBlock(`${FENCE}pij\nsend a b`, false)).toEqual({ hidden: false });
	});

	it("draws a pij example inside another fence as written (review of #212, F001)", () => {
		const quoted = `${FENCE}text\n${FENCE}pij\nsend peer hi\n${FENCE}`;
		expect(hideTrailingBlock(quoted, false)).toEqual({ hidden: false });
		expect(hideTrailingBlock(`${FENCE}text\n${FENCE}pij\nsend peer h`, true)).toEqual({
			hidden: false,
		});
		expect(mayEndWithBlock(quoted)).toBe(true);
		expect(
			hideTrailingBlock(`\`${FENCE}md\n${FENCE}pij\nsend a b\n${FENCE}\n\`${FENCE}`, false),
		).toEqual({ hidden: false });
		const after = `\`${FENCE}md\n${FENCE}pij\nsend a quoted\n${FENCE}\n\`${FENCE}\n${FENCE}pij\nsend b real\n${FENCE}`;
		expect(hideTrailingBlock(after, false)).toEqual({
			hidden: true,
			shown: `\`${FENCE}md\n${FENCE}pij\nsend a quoted\n${FENCE}\n\`${FENCE}\n\n⚙ pij: 1 command(s) handed to pij`,
			commands: 1,
		});
	});
});

describe("stopDecision", () => {
	it("shows the summary and never blocks on success", () => {
		expect(stopDecision(result({}), false)).toEqual({
			line: { text: "pij ✓ 2 sent", level: "info" },
		});
	});

	it("blocks with the follow-up only when the stop hook is not active", () => {
		const failed = result({
			outcome: "refused",
			ran: 1,
			summary: "pij ✗ line 2 refused: E-RS-COLD-WAKE (1 ran, 0 not run)",
			followup: "[pij] Your end-of-turn pij block stopped at a refusal.",
		});
		expect(stopDecision(failed, false).block).toBe(failed.followup);
		expect(stopDecision(failed, true)).toEqual({
			line: { text: failed.summary, level: "warning" },
		});
	});

	it("does nothing when there was no block", () => {
		expect(stopDecision(result({ outcome: "none", summary: null, ran: 0 }), false)).toEqual({});
	});

	it("says nothing ran when the daemon refused the call, and to check first when the outcome is unknown", () => {
		const refused = noResultDecision("refused", "E-RS-ARG: bad body", false);
		expect(refused.line?.text).toBe(
			"pij ✗ block not run: the daemon refused it (E-RS-ARG: bad body)",
		);
		expect(refused.block).toContain("Nothing was sent.");
		const unknown = noResultDecision("unknown", "timeout", false);
		expect(unknown.line?.text).toBe("pij ? block outcome unknown: timeout");
		expect(unknown.block).toContain("may have run");
		expect(noResultDecision("unknown", "timeout", true).block).toBeUndefined();
	});

	it("clips and escapes a transport reason to one line (review of #212, S212-N001)", () => {
		const forged = noResultDecision("unknown", "lost\u001b[31m\nforged line", false);
		expect(forged.line?.text).toBe("pij ? block outcome unknown: lost\\u{1b}[31m");
		expect(forged.block).not.toContain("forged");
		const long = noResultDecision("refused", "x".repeat(500), false);
		expect(long.line?.text).toBe(
			`pij ✗ block not run: the daemon refused it (${"x".repeat(120)}…)`,
		);
	});
});

describe("decodeTurnEndResult", () => {
	it("refuses anything that is not the contract", () => {
		expect(() => decodeTurnEndResult(null)).toThrow();
		expect(() => decodeTurnEndResult({ outcome: "maybe", lines: [], ran: 0 })).toThrow();
		expect(decodeTurnEndResult(result({}))).toEqual(result({}));
	});
});

describe("TurnEndDisplay", () => {
	it("draws a reply that ended in a tool call as written, after a redraw", () => {
		const display = new TurnEndDisplay();
		const message = { timestamp: 7 };
		expect(display.render({ message, text: BLOCK, streaming: true })).toContain("⚙ pij");
		expect(
			display.messageEnded({
				role: "assistant",
				stopReason: "toolUse",
				timestamp: 7,
				content: [{ type: "text", text: BLOCK }],
			}),
		).toBe(7);
		expect(display.render({ message, text: BLOCK, streaming: false })).toBeUndefined();
	});

	it("keeps hiding a reply that stopped cleanly", () => {
		const display = new TurnEndDisplay();
		expect(
			display.messageEnded({
				role: "assistant",
				stopReason: "stop",
				timestamp: 8,
				content: [{ type: "text", text: BLOCK }],
			}),
		).toBeUndefined();
		expect(display.render({ message: { timestamp: 8 }, text: BLOCK, streaming: false })).toBe(
			"Done.\n\n⚙ pij: 2 command(s) handed to pij",
		);
	});

	it("draws a reply without an opener as written, and answers a redraw from memory (P212-N001)", () => {
		const display = new TurnEndDisplay();
		const long = `${"prose ".repeat(20_000)}\n${FENCE}js\nx\n${FENCE}`;
		expect(display.render({ message: {}, text: long, streaming: false })).toBeUndefined();
		const first = display.render({ message: {}, text: BLOCK, streaming: false });
		expect(display.render({ message: {}, text: BLOCK, streaming: false })).toBe(first);
		// Streaming and settled draws of the same text are remembered apart.
		const streamed = `${BLOCK}more`;
		expect(display.render({ message: {}, text: streamed, streaming: true })).toBeUndefined();
		for (let i = 0; i < 40; i++)
			display.render({ message: {}, text: `${BLOCK}${i}`, streaming: false });
		expect(display.render({ message: {}, text: BLOCK, streaming: false })).toBe(first);
	});
});
