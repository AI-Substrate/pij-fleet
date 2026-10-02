// pij-messaging — PiRuntimePort adapter.
//
// ⚠️ THE ONLY FILE under .pi/extensions/pij/ that imports @earendil-works/*.
// Everything in core/ stays pi-free (Patterns P2/P9); this adapter is the
// single seam where the pure core meets the live pi session.
//
// OMP custom messages bypass its user-message editor clearing; Pi retains its
// existing user-message API. Both start a turn when idle and steer when busy.

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { PiRuntimePort } from "../core/ports.js";

/** The command-context-only session-control ops, captured from pi's
 *  ExtensionCommandContext the moment the `/pij` command runs (the single
 *  instant pi exposes them). index.ts owns the live holder; null until armed. */
export interface CommandControl {
	newSession(): Promise<{ cancelled: boolean }>;
	reload(): Promise<void>;
}

type RuntimeContext = Pick<ExtensionContext, "isIdle" | "compact"> & {
	readonly ui: Pick<ExtensionContext["ui"], "setStatus" | "notify">;
};

export class PiRuntimeAdapter implements PiRuntimePort {
	constructor(
		private readonly pi: Pick<ExtensionAPI, "sendUserMessage" | "sendMessage">,
		private readonly ctx: RuntimeContext,
		private readonly runtimeBin: "pi" | "omp",
		/** Reads the currently-armed command control, or undefined if no `/pij`
		 *  invocation has captured one yet (or it was consumed by a prior op). */
		private readonly controlRef: () => CommandControl | undefined = () => undefined,
	) {}

	isIdle(): boolean {
		return this.ctx.isIdle();
	}

	/** Arrival becomes OBSERVABLE here. Delivery to a busy seat already happened;
	 * a steered message just is not on screen until the step boundary, which on a
	 * slow model is minutes of the human believing nothing came. */
	notify(text: string, level: "info" | "warning" | "error" = "info"): void {
		this.ctx.ui.notify(text, level);
	}

	setStatus(text: string | undefined): void {
		this.ctx.ui.setStatus("pij-mail", text);
	}

	setFyiStatus(text: string | undefined): void {
		this.ctx.ui.setStatus("pij-fyi", text);
	}

	inject(
		text: string,
		mode: "immediate" | "steer",
		messageId?: string,
		resendAttempt?: number,
	): void {
		if (resendAttempt !== undefined && messageId !== undefined) {
			// A swallowed custom steer has no native receipt. The prompt path is
			// deliberately distinct, with an id that survives user message_start.
			const prompt = `[pij resend ${resendAttempt}]\n[pijMessageId:${encodeURIComponent(messageId)}]\n${text}`;
			if (mode === "steer") this.pi.sendUserMessage(prompt, { deliverAs: "steer" });
			else this.pi.sendUserMessage(prompt);
		} else if (this.runtimeBin === "omp" || messageId !== undefined) {
			// Peer messages carry native consumption identity, never user attribution:
			// OMP must not restore them into the human's draft on Escape.
			// Test-only fault injection for isolated-daemon swallowed-custom-mail proofs.
			if (
				this.runtimeBin === "omp" &&
				messageId !== undefined &&
				process.env.PIJ_TEST_SWALLOW_INJECT === "1"
			)
				return;
			this.pi.sendMessage(
				{
					customType: "pij",
					content: text,
					display: true,
					...(messageId === undefined ? {} : { details: { pijMessageId: messageId } }),
				},
				{ triggerTurn: true, deliverAs: "steer" },
			);
		} else if (mode === "steer") {
			this.pi.sendUserMessage(text, { deliverAs: "steer" });
		} else {
			this.pi.sendUserMessage(text);
		}
	}

	compact(): Promise<void> {
		return new Promise<void>((resolve, reject) => {
			void Promise.resolve(
				this.ctx.compact({ onComplete: () => resolve(), onError: reject }),
			).catch(reject);
		});
	}

	control(command: "new" | "reload"): boolean | Promise<boolean> {
		const c = this.controlRef();
		if (!c) return false;
		if (command === "reload" && !this.ctx.isIdle()) {
			return Promise.reject(new Error("reload refused while the runtime is busy"));
		}
		if (command === "new") {
			return c.newSession().then(({ cancelled }) => {
				if (cancelled) throw new Error("new session was cancelled by the runtime");
				return true;
			});
		}
		return c.reload().then(() => true);
	}
}

const RELOAD_COMPLETED = "pij:reload-completed";

/** OMP's supported lifecycle differs from Pi's upstream extension typings. */
interface OmpReloadLifecycle {
	on(event: "session_switch", handler: (event: { readonly reason: string }) => void): void;
}

export function publishReloadCompleted(pi: Pick<ExtensionAPI, "events">): void {
	pi.events.emit(RELOAD_COMPLETED, undefined);
}

export function registerOmpReloadCompletion(pi: Pick<ExtensionAPI, "on" | "events">): void {
	(pi as unknown as OmpReloadLifecycle).on("session_switch", (event) => {
		if (event.reason === "resume") publishReloadCompleted(pi);
	});
}

/** Both runtimes await their lifecycle handlers before reload() settles. The
 * shared extension bus survives Pi resource reload, unlike this module's state. */
export async function reloadWithCompletion(
	pi: Pick<ExtensionAPI, "events">,
	reload: () => Promise<void>,
): Promise<void> {
	let completed = false;
	const unsubscribe = pi.events.on(RELOAD_COMPLETED, () => {
		completed = true;
	});
	try {
		await reload();
		if (!completed)
			throw new Error(
				"reload did not complete; the runtime was busy, compacting, or refused the operation",
			);
	} finally {
		unsubscribe();
	}
}
