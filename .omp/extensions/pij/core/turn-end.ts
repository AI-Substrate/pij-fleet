// pij-messaging — end-of-turn pij blocks (plan 226), the OMP side's decisions.
//
// Pure (P2): nothing here imports a runtime. A seat ends its reply with a
// fenced `pij` block; at session_stop the extension hands the final text to the
// daemon's POST /v1/turn-end, which validates and runs it. This module decides
// when to call (a cheap tail check), what the human sees (one line, and the
// block hidden on screen), and whether the seat gets its one follow-up turn.

/** The fence's language tag. Mirrors `pij_core::turn_end::FENCE_TAG`. */
export const TURN_END_FENCE_TAG = "pij";
/** The one opener a block starts with; a reply without it holds no block. */
export const TURN_END_OPENER = `\`\`\`${TURN_END_FENCE_TAG}`;
/** The custom message that carries the one result line into the transcript. */
export const TURN_END_CUSTOM_TYPE = "pij-turn-end";
/** Under OMP's 30 s extension-handler budget, so the answer arrives in time. */
export const TURN_END_CALL_TIMEOUT_MS = 25_000;
/** A notice raised during the settle never draws: wait for idle, bounded. */
export const TURN_END_IDLE_POLL_MS = 50;
export const TURN_END_IDLE_POLL_LIMIT = 40;
/** How many tool-call replies the renderer remembers as never-run. */
export const TURN_END_NO_RUN_MEMORY = 64;
/** How many drawn texts holding an opener the renderer remembers. */
export const TURN_END_RENDER_MEMORY = 32;
/** How much of a reason one line carries. Mirrors `SUMMARY_REASON_CHARS`. */
export const TURN_END_REASON_CHARS = 120;

/** One command's result, as `POST /v1/turn-end` answers it. */
export interface TurnEndLineWire {
	readonly line: number;
	readonly command: string;
	readonly verb: "send" | "output" | "requests close" | null;
	readonly status: "ran" | "refused" | "invalid" | "not_run";
	readonly detail: string;
	readonly code: string | null;
	readonly result?: unknown;
}

/** `POST /v1/turn-end`'s `data`. */
export interface TurnEndResultWire {
	readonly outcome: "none" | "ran" | "invalid" | "refused";
	readonly lines: readonly TurnEndLineWire[];
	readonly ran: number;
	readonly summary: string | null;
	readonly followup: string | null;
}

/** The request body. `caller` is the daemon's CallerContext wire object. */
export interface TurnEndRequestWire {
	readonly text: string;
	readonly caller: {
		readonly PIJ_SESSION_ID: string;
		readonly TMUX_PANE?: string;
		readonly HARNESS_SESSION_ID?: string;
		readonly cwd?: string;
	};
	readonly via: "hook:omp" | "hook:pi";
}

/** Decode `data`, refusing anything that is not the contract. */
export function decodeTurnEndResult(data: unknown): TurnEndResultWire {
	if (typeof data !== "object" || data === null || Array.isArray(data))
		throw new Error("pij turn-end returned a non-object");
	const row = data as Record<string, unknown>;
	const outcomes = ["none", "ran", "invalid", "refused"];
	if (
		typeof row.outcome !== "string" ||
		!outcomes.includes(row.outcome) ||
		!Array.isArray(row.lines) ||
		typeof row.ran !== "number" ||
		!(row.summary === null || typeof row.summary === "string") ||
		!(row.followup === null || typeof row.followup === "string")
	)
		throw new Error("pij turn-end returned an unexpected shape");
	return row as unknown as TurnEndResultWire;
}

/** A Markdown code fence line. Mirrors `pij_core::turn_end`'s `Fence`. */
interface Fence {
	readonly marker: "`" | "~";
	readonly len: number;
	readonly info: string;
}

function fence(line: string): Fence | undefined {
	const trimmed = line.trim();
	const marker = trimmed[0];
	if (marker !== "`" && marker !== "~") return undefined;
	let len = 0;
	while (trimmed[len] === marker) len++;
	const info = trimmed.slice(len);
	if (len < 3 || (marker === "`" && info.includes("`"))) return undefined;
	return { marker, len, info };
}

function isPij(opener: Fence): boolean {
	return opener.marker === "`" && opener.info === TURN_END_FENCE_TAG;
}

function isOpenFence(line: string): boolean {
	const found = fence(line);
	return found !== undefined && isPij(found);
}

/**
 * Where the text's fences stand, tracked from its start: the last closed block
 * and the fence still open at the end. A `pij` line inside an open fence is
 * that fence's content. Mirrors `pij_core::turn_end`'s `trailing_block_body`.
 */
function scanFences(lines: readonly string[]): {
	readonly last?: { readonly start: number; readonly close: number; readonly pij: boolean };
	readonly open?: { readonly start: number; readonly pij: boolean };
} {
	let open: { start: number; opener: Fence } | undefined;
	let last: { start: number; close: number; pij: boolean } | undefined;
	for (const [index, line] of lines.entries()) {
		const found = fence(line);
		if (found === undefined) continue;
		if (open === undefined) {
			open = { start: index, opener: found };
		} else if (
			found.marker === open.opener.marker &&
			found.len >= open.opener.len &&
			found.info === ""
		) {
			last = { start: open.start, close: index, pij: isPij(open.opener) };
			open = undefined;
		}
	}
	return {
		...(last ? { last } : {}),
		...(open ? { open: { start: open.start, pij: isPij(open.opener) } } : {}),
	};
}

/**
 * The cheap check before calling the daemon: the trimmed text ends with a
 * closing fence and some line opens a `pij` fence. The daemon's parse decides.
 */
export function mayEndWithBlock(text: string): boolean {
	return (
		text.trimEnd().endsWith("```") &&
		text.includes(TURN_END_OPENER) &&
		text.split("\n").some(isOpenFence)
	);
}

/** The text blocks of an assistant message, joined. */
export function textOf(message: { readonly content?: unknown } | undefined): string {
	const content = message?.content;
	if (typeof content === "string") return content;
	if (!Array.isArray(content)) return "";
	return content
		.filter(
			(block): block is { type: "text"; text: string } =>
				typeof block === "object" &&
				block !== null &&
				(block as { type?: unknown }).type === "text" &&
				typeof (block as { text?: unknown }).text === "string",
		)
		.map((block) => block.text)
		.join("\n");
}

export type HiddenBlock =
	| { readonly hidden: false }
	| { readonly hidden: true; readonly shown: string; readonly commands: number };

/** A half-typed opener at the very end of a streaming reply. */
const PARTIAL_OPENER = /\n[ \t]*`{1,3}(?:p|pi|pij)?[ \t]*$/;
/** Enough of the tail to hold a half-typed opener and its newline. */
const PARTIAL_TAIL_CHARS = 24;

/**
 * Display only: replace the `pij` block that ends `text` with one line. While
 * streaming, an unclosed block is hidden as it grows, and a half-typed opening
 * fence is held back so it never flashes. The renderer gets the whole text so
 * far on every draw, so no state is carried between chunks. A text without the
 * opener costs one `includes` and a look at its tail.
 */
export function hideTrailingBlock(text: string, streaming: boolean): HiddenBlock {
	if (text.includes(TURN_END_OPENER)) {
		const lines = text.split("\n");
		const { last, open } = scanFences(lines);
		const hide = (start: number, body: readonly string[], closed: boolean): HiddenBlock => {
			const commands = body.filter((line) => {
				const trimmed = line.trim();
				return trimmed !== "" && !trimmed.startsWith("#") && !trimmed.startsWith("`");
			}).length;
			const head = lines.slice(0, start).join("\n").trimEnd();
			const label = closed
				? `⚙ pij: ${commands} command(s) handed to pij`
				: `⚙ pij: ${commands} command(s)…`;
			return { hidden: true, shown: head === "" ? label : `${head}\n\n${label}`, commands };
		};
		if (open?.pij && streaming) return hide(open.start, lines.slice(open.start + 1), false);
		if (
			open === undefined &&
			last?.pij &&
			lines.slice(last.close + 1).every((line) => line.trim() === "")
		)
			return hide(last.start, lines.slice(last.start + 1, last.close), true);
	}
	if (!streaming) return { hidden: false };
	const held = PARTIAL_OPENER.exec(text.slice(-PARTIAL_TAIL_CHARS));
	if (held === null) return { hidden: false };
	return { hidden: true, shown: text.slice(0, text.length - held[0].length), commands: 0 };
}

/** What the extension does with a turn-end answer. */
export interface StopDecision {
	/** One line for the human, drawn once the session is idle. */
	readonly line?: { readonly text: string; readonly level: "info" | "warning" };
	/** The seat's one follow-up turn; only when its stop hook is not active. */
	readonly block?: string;
}

/** Decide from the daemon's answer. The daemon never sees stop_hook_active. */
export function stopDecision(result: TurnEndResultWire, stopHookActive: boolean): StopDecision {
	if (result.outcome === "none" || result.summary === null) return {};
	if (result.followup === null) return { line: { text: result.summary, level: "info" } };
	return {
		line: { text: result.summary, level: "warning" },
		...(stopHookActive ? {} : { block: result.followup }),
	};
}

/**
 * A transport or refusal reason for one line: its first line, control
 * characters escaped, cut at TURN_END_REASON_CHARS. The daemon fences its own
 * reasons; this is the path where no daemon answer came back.
 */
export function clipReason(reason: string): string {
	const first = (reason.split("\n")[0] ?? "").trim();
	// biome-ignore lint/suspicious/noControlCharactersInRegex: escaping them is the point
	const escaped = first.replace(/[\u0000-\u001f\u007f-\u009f]/g, (ch) => {
		return `\\u{${ch.charCodeAt(0).toString(16)}}`;
	});
	const chars = Array.from(escaped);
	return chars.length <= TURN_END_REASON_CHARS
		? escaped
		: `${chars.slice(0, TURN_END_REASON_CHARS).join("")}…`;
}

/**
 * No result came back. `refused`: the daemon (or this seat) refused the call,
 * so nothing ran. `unknown`: the call was lost, timed out or failed inside the
 * daemon, so some commands may have run and the seat must check before
 * resending. The same wording as the Claude and Copilot hooks.
 */
export function noResultDecision(
	kind: "refused" | "unknown",
	rawReason: string,
	stopHookActive: boolean,
): StopDecision {
	const reason = clipReason(rawReason);
	const [line, block] =
		kind === "refused"
			? [
					`pij ✗ block not run: the daemon refused it (${reason})`,
					[
						`[pij] Your end-of-turn pij block did not run: the daemon refused it (${reason}). Nothing was sent.`,
						"Fix it and end your reply with a corrected pij block, or say why not.",
					].join("\n"),
				]
			: [
					`pij ? block outcome unknown: ${reason}`,
					[
						`[pij] Your end-of-turn pij block's outcome is unknown (${reason}): some of its commands may have run.`,
						"Check before you resend any of them, or say why not.",
					].join("\n"),
				];
	return { line: { text: line, level: "warning" }, ...(stopHookActive ? {} : { block }) };
}

/** The assistant-text renderer's state: replies whose block will never run. */
export class TurnEndDisplay {
	private readonly noRun: number[] = [];
	/** Drawn texts holding an opener, oldest first: while streaming, and settled. */
	private readonly drawnStreaming = new Map<string, string | undefined>();
	private readonly drawnSettled = new Map<string, string | undefined>();

	/**
	 * A reply ended. One that ended any way but "stop" (a tool call, an abort,
	 * an error) never runs its block, so it is drawn raw again: returns the
	 * timestamp to redraw, or undefined.
	 */
	messageEnded(message: {
		readonly role?: string;
		readonly stopReason?: string;
		readonly timestamp?: number;
		readonly content?: unknown;
	}): number | undefined {
		if (message.role !== "assistant" || message.stopReason === "stop") return undefined;
		if (typeof message.timestamp !== "number") return undefined;
		if (!textOf(message).includes(TURN_END_OPENER)) return undefined;
		this.noRun.push(message.timestamp);
		if (this.noRun.length > TURN_END_NO_RUN_MEMORY) this.noRun.shift();
		return message.timestamp;
	}

	/**
	 * The markdown to draw for one assistant text block, or undefined for as
	 * written. OMP redraws every visible reply on each frame, so a text with
	 * an opener is scanned once and remembered (review of #212, P212-N001).
	 */
	render(context: {
		readonly message: { readonly timestamp?: number };
		readonly text: string;
		readonly streaming: boolean;
	}): string | undefined {
		const stamp = context.message.timestamp;
		if (typeof stamp === "number" && this.noRun.includes(stamp)) return undefined;
		const { text, streaming } = context;
		if (!text.includes(TURN_END_OPENER)) {
			const hidden = hideTrailingBlock(text, streaming);
			return hidden.hidden ? hidden.shown : undefined;
		}
		const drawn = streaming ? this.drawnStreaming : this.drawnSettled;
		if (drawn.has(text)) return drawn.get(text);
		const hidden = hideTrailingBlock(text, streaming);
		const shown = hidden.hidden ? hidden.shown : undefined;
		drawn.set(text, shown);
		if (drawn.size > TURN_END_RENDER_MEMORY) {
			const oldest = drawn.keys().next();
			if (!oldest.done) drawn.delete(oldest.value);
		}
		return shown;
	}
}
