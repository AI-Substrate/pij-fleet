// harness/driver/session.ts
//
// High-level Session class — the ergonomic wrapper most callers use.
// Wraps a Target with state-aware methods. Transcribed from workshop 001
// § Module: session.ts. Encodes PR-01/PR-02/PR-04/PR-07 (pi rendering
// surface) and closes IA-02/IA-04/IA-08 (smoke brittleness).

import { DriverAssertionError, DriverIdleTimeoutError } from "./errors.js";
import {
	assertAlive,
	type BootOpts,
	boot,
	capture,
	type Key,
	paste,
	press,
	record as recordPane,
	type Target,
	targetStr,
	teardown,
	type as typeText,
} from "./tmux.js";

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

// ─── Step types (IA-03) ─────────────────────────────────────────────────────

export type Step =
	| { kind: "type"; text: string; press?: Key; expect?: RegExp; expectTimeoutMs?: number }
	| { kind: "press"; key: Key; n?: number; expect?: RegExp; expectTimeoutMs?: number }
	| { kind: "paste"; data: string; expect?: RegExp; expectTimeoutMs?: number }
	| { kind: "wait"; quietMs?: number; signal?: RegExp; timeoutMs?: number }
	| { kind: "sleep"; ms: number } // explicit escape hatch — discouraged
	| { kind: "capture"; name: string }; // attach named capture to report

// ─── Readiness opts (PR-07) ─────────────────────────────────────────────────

export interface WaitIdleOpts {
	/** Bottom-line prompt regex. Default: `/^>\s/m` — pi's prompt (PR-01). */
	promptRe?: RegExp;
	/** Spinner glyphs to wait OUT. Default: pi's Braille frames (PR-07). */
	spinnerRe?: RegExp;
	/** Footer context-percent token. Default: `/\d+(\.\d+)?%\//` (PR-02). */
	contextRe?: RegExp;
	/** Polling interval. Default 250ms. */
	quietMs?: number;
	/** Hard ceiling. Default 5000ms; bump to 60000 for `/compact` / `/reload`. */
	timeoutMs?: number;
	/** Capture window in lines. Default 50. */
	scrollback?: number;
}

// Pi-ready signal. Two patterns are accepted:
//   - Legacy `> ` prompt at line start (pre-TUI pi, kept for safety)
//   - Modern pi TUI footer: "<model-id> • <tier>" — pi v0.74 always
//     ends its status footer with e.g. "gpt-5.5 • medium" or
//     "claude-opus • high". The `•` bullet is unique to pi's footer
//     (it is not used in pi's banner, slash output, or extension UI).
// The check is against the LAST non-empty line, so a match here means
// "pi has finished rendering and the bottom line is the status footer".
// PR-01 in the research dossier captured the legacy `>` shape; the
// bullet alternative was discovered during T005 manual verification
// (pi v0.74.0 capture had no `>` glyph anywhere on screen).
// OMP (the default driven harness) renders its composer as a bare `❯` line
// between rules, with the status footer above it.
export const DEFAULT_PROMPT_RE = /^>\s|\s•\s\w|^❯(\s|$)/;
export const DEFAULT_SPINNER_RE = /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/;
export const DEFAULT_CONTEXT_RE = /\d+(\.\d+)?%\//;

const RISKY_PAYLOAD_RE = /[\n"`$;]|^-/; // workshop 001 D6: auto-route to paste()

// ─── Session class ──────────────────────────────────────────────────────────

export class Session {
	private readonly target: Target;
	private readonly priorSteps: Step[] = [];
	private readonly captures: Record<string, string> = {};
	private recorder?: { stop: () => void };

	private constructor(target: Target) {
		this.target = target;
	}

	/** Boot a new tmux session running `cmd`. */
	static async start(opts: BootOpts & { recordPath?: string }): Promise<Session> {
		const target = boot(opts);
		const s = new Session(target);
		if (opts.recordPath) s.recorder = recordPane(target, opts.recordPath);
		return s;
	}

	/** Test-only escape hatch: wrap an existing Target without booting. */
	static fromTarget(target: Target): Session {
		return new Session(target);
	}

	get id(): string {
		return targetStr(this.target);
	}

	/** Wait until pi is idle: output-stable + prompt visible + no spinner (PR-07). */
	async waitIdle(opts: WaitIdleOpts = {}): Promise<string> {
		const promptRe = opts.promptRe ?? DEFAULT_PROMPT_RE;
		const spinnerRe = opts.spinnerRe ?? DEFAULT_SPINNER_RE;
		const contextRe = opts.contextRe ?? DEFAULT_CONTEXT_RE;
		const quietMs = opts.quietMs ?? 250;
		const timeoutMs = opts.timeoutMs ?? 5000;
		const scrollback = opts.scrollback ?? 50;

		const deadline = Date.now() + timeoutMs;
		let last = "";
		let stable = "";

		while (Date.now() < deadline) {
			const now = capture(this.target, { scrollback, join: true });
			if (now === last && now !== "") {
				const last3 = now.split("\n").slice(-3).join("\n");
				const lines = now.split("\n");
				const promptVisible = lines
					.filter((line) => line.trim() !== "")
					.slice(-5)
					.some((line) => {
						promptRe.lastIndex = 0;
						return promptRe.test(line);
					});
				if (contextRe.test(now) && promptVisible && !spinnerRe.test(last3)) {
					return now;
				}
				stable = now;
			}
			last = now;
			await sleep(quietMs);
		}
		throw new DriverIdleTimeoutError(this.target, stable || last, timeoutMs);
	}

	/** Type-then-confirm — the atomic primitive (TC-12). Auto-routes risky payloads. */
	async run(
		input: string,
		expect: RegExp,
		opts: { timeoutMs?: number; press?: Key } = {},
	): Promise<string> {
		const t0 = Date.now();
		const pressKey = opts.press ?? "Enter";
		const timeout = opts.timeoutMs ?? 5000;
		const step: Step = {
			kind: "type",
			text: input,
			press: pressKey,
			expect,
			expectTimeoutMs: timeout,
		};

		assertAlive(this.target);

		if (RISKY_PAYLOAD_RE.test(input)) paste(this.target, input);
		else typeText(this.target, input);
		press(this.target, pressKey);

		const deadline = t0 + timeout;
		let pane = "";
		while (Date.now() < deadline) {
			pane = capture(this.target, { scrollback: 2000, join: true });
			if (expect.test(pane)) {
				this.priorSteps.push(step);
				return pane;
			}
			await sleep(150);
		}
		throw this.assertionFailure(step, expect, pane, t0);
	}

	/** Execute one Step. Adapter between scenario JSON and the underlying primitives. */
	async execute(step: Step): Promise<void> {
		const t0 = Date.now();
		switch (step.kind) {
			case "type": {
				if (RISKY_PAYLOAD_RE.test(step.text)) paste(this.target, step.text);
				else typeText(this.target, step.text);
				if (step.press) press(this.target, step.press);
				if (step.expect)
					await this.expectMatch(step, step.expect, step.expectTimeoutMs ?? 5000, t0);
				break;
			}
			case "press": {
				press(this.target, step.key, step.n);
				if (step.expect)
					await this.expectMatch(step, step.expect, step.expectTimeoutMs ?? 5000, t0);
				break;
			}
			case "paste": {
				paste(this.target, step.data);
				if (step.expect)
					await this.expectMatch(step, step.expect, step.expectTimeoutMs ?? 5000, t0);
				break;
			}
			case "wait": {
				await this.waitIdle({
					quietMs: step.quietMs,
					promptRe: step.signal,
					timeoutMs: step.timeoutMs,
				});
				break;
			}
			case "sleep": {
				await sleep(step.ms);
				break;
			}
			case "capture": {
				this.captures[step.name] = capture(this.target, { scrollback: 2000, join: true });
				break;
			}
		}
		this.priorSteps.push(step);
	}

	capturedNamed(): Record<string, string> {
		return { ...this.captures };
	}

	/**
	 * Capture the current pane content as a string (full scrollback by default).
	 * Used by helpers that need a baseline snapshot before sending a command
	 * (e.g. `compactAndAssert`'s freshness gate — F004 mitigation).
	 */
	snapshot(scrollback: number = 2000): string {
		return capture(this.target, { scrollback, join: true });
	}

	teardown(): void {
		this.recorder?.stop();
		teardown(this.target);
	}

	// ─── private ──────────────────────────────────────────────────────────────

	private async expectMatch(step: Step, re: RegExp, timeoutMs: number, t0: number): Promise<void> {
		const deadline = t0 + timeoutMs;
		let pane = "";
		while (Date.now() < deadline) {
			pane = capture(this.target, { scrollback: 2000, join: true });
			if (re.test(pane)) return;
			await sleep(150);
		}
		throw this.assertionFailure(step, re, pane, t0);
	}

	private assertionFailure(
		step: Step,
		expected: RegExp,
		actual: string,
		t0: number,
	): DriverAssertionError {
		const scrollback = capture(this.target, { scrollback: 5000, join: true });
		const status = scrollback.split("\n").slice(-3).join("\n");
		return new DriverAssertionError({
			target: this.target,
			step,
			expected,
			actual,
			scrollback,
			status,
			priorSteps: [...this.priorSteps],
			durationMs: Date.now() - t0,
		});
	}
}
