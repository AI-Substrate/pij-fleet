// pij-messaging — end-of-turn pij blocks on OMP (plan 226).
//
// OMP's own Stop hook is `session_stop`: it fires once, at the terminal settle
// of the main agent, with the final text-only reply (never for a reply that
// ended in a tool call, an abort, a retry or a subagent). These hooks are
// OMP-only and absent from Pi's upstream typings, so they are typed here
// structurally, as registerOmpReloadCompletion does.
//
// Cost: in process. One daemon request per turn end whose reply ends with a
// `pij` fence, none otherwise; the renderer is a function call per draw. The
// idle wait is a bounded 50 ms timer, only after a block ran.

import { DaemonRefusalError } from "../core/daemon-wire.js";
import {
	mayEndWithBlock,
	noResultDecision,
	type StopDecision,
	stopDecision,
	TURN_END_CUSTOM_TYPE,
	TURN_END_IDLE_POLL_LIMIT,
	TURN_END_IDLE_POLL_MS,
	TurnEndDisplay,
	type TurnEndResultWire,
	textOf,
} from "../core/turn-end.js";

interface OmpMessage {
	readonly role?: string;
	readonly stopReason?: string;
	readonly timestamp?: number;
	readonly content?: unknown;
}

interface OmpContext {
	readonly ui: {
		notify(message: string, type?: "info" | "warning" | "error"): void;
		rerenderMessage?(timestamp: number): void;
	};
	isIdle(): boolean;
}

interface OmpStopEvent {
	readonly last_assistant_message?: OmpMessage;
	readonly stop_hook_active: boolean;
}

type OmpStopResult = { readonly decision: "block"; readonly reason: string } | undefined;

interface OmpComponent {
	render(width: number): string[];
	invalidate(): void;
}

/** The OMP extension surface this module uses (OMP 18.14). */
interface OmpTurnEndApi {
	on(
		event: "session_stop",
		handler: (event: OmpStopEvent, ctx: OmpContext) => Promise<OmpStopResult>,
	): void;
	on(
		event: "message_end",
		handler: (event: { readonly message: OmpMessage }, ctx: OmpContext) => void,
	): void;
	on(
		event: "agent_end",
		handler: (event: { readonly willContinue?: boolean }, ctx: OmpContext) => void,
	): void;
	registerAssistantTextRenderer?(
		renderer: (context: {
			readonly message: OmpMessage;
			readonly text: string;
			readonly streaming: boolean;
		}) => string | undefined,
	): void;
	registerMessageRenderer(
		customType: string,
		renderer: (
			message: { readonly content: unknown },
			options: unknown,
			theme: { fg(color: string, text: string): string },
		) => OmpComponent | undefined,
	): void;
	sendMessage(
		message: { readonly customType: string; readonly content: string; readonly display: boolean },
		options?: { readonly triggerTurn?: boolean },
	): void;
}

/** Runs one turn end through the daemon (the Rust runtime session). */
export interface TurnEndRunner {
	turnEnd(text: string): Promise<TurnEndResultWire>;
}

/**
 * Wire end-of-turn blocks into an OMP session. `runner` is read per stop, so a
 * reload's new session is used. OMP never raises `session_stop` for a
 * subagent, so in-process task sessions run nothing.
 */
export function registerOmpTurnEnd(pi: unknown, runner: () => TurnEndRunner | undefined): void {
	const omp = pi as OmpTurnEndApi;
	const display = new TurnEndDisplay();
	let pending: NonNullable<StopDecision["line"]>[] = [];

	// The renderer draws one line where the block is; the session, exports and
	// model context keep the reply as written. Needs extensionRenderers.assistantText.
	omp.registerAssistantTextRenderer?.((context) => display.render(context));
	omp.registerMessageRenderer(TURN_END_CUSTOM_TYPE, (message, _options, theme) => {
		const text = typeof message.content === "string" ? message.content : "";
		const color = text.startsWith("pij ✗") ? "warning" : "dim";
		return {
			render: (width: number) => [` ${theme.fg(color, text.slice(0, Math.max(0, width - 2)))}`],
			invalidate: () => {},
		};
	});

	omp.on("message_end", (event, ctx) => {
		const stamp = display.messageEnded(event.message);
		if (stamp !== undefined) ctx.ui.rerenderMessage?.(stamp);
	});

	omp.on("session_stop", async (event) => {
		const message = event.last_assistant_message;
		if (message?.stopReason !== "stop") return undefined;
		const text = textOf(message);
		if (!mayEndWithBlock(text)) return undefined;
		const session = runner();
		let decision: StopDecision;
		if (session === undefined) {
			decision = noResultDecision("refused", "this seat is not registered", event.stop_hook_active);
		} else {
			try {
				decision = stopDecision(await session.turnEnd(text), event.stop_hook_active);
			} catch (error) {
				// A refusal or an auth failure answered before anything ran; a
				// timeout, a lost connection or a 5xx may have run some of it.
				const refused =
					error instanceof DaemonRefusalError &&
					(error.kind === "refused" || error.kind === "auth");
				decision = noResultDecision(
					refused ? "refused" : "unknown",
					error instanceof Error ? error.message : String(error),
					event.stop_hook_active,
				);
			}
		}
		if (decision.line !== undefined) pending.push(decision.line);
		return decision.block === undefined ? undefined : { decision: "block", reason: decision.block };
	});

	// A notice raised during the settle never draws; the line is a custom
	// message posted once the session is idle, after the whole run settles.
	omp.on("agent_end", (event, ctx) => {
		if (event.willContinue === true || pending.length === 0) return;
		const flush = (tries: number): void => {
			if (!ctx.isIdle() && tries < TURN_END_IDLE_POLL_LIMIT) {
				setTimeout(() => flush(tries + 1), TURN_END_IDLE_POLL_MS);
				return;
			}
			const lines = pending;
			pending = [];
			for (const line of lines) {
				omp.sendMessage(
					{ customType: TURN_END_CUSTOM_TYPE, content: line.text, display: true },
					{ triggerTurn: false },
				);
			}
		};
		flush(0);
	});
}
