// harness/driver/errors.ts
//
// Structured driver error hierarchy. Each subclass carries enough context
// that a human or agent reading a stack trace doesn't need to spelunk live
// state. Per workshop 001 § Module: errors.ts — uses `import type` for
// forward decls so there's no circular runtime import.

import type { Step } from "./session.js";
import type { BootOpts, PaneInfo, Target } from "./tmux.js";

export class DriverError extends Error {
	public override readonly cause?: Error;

	constructor(message: string, cause?: Error) {
		super(message);
		this.name = "DriverError";
		this.cause = cause;
	}
}

export class DriverBootError extends DriverError {
	public readonly opts: BootOpts;

	constructor(opts: BootOpts, cause: Error) {
		super(`boot failed: session=${opts.session} cmd=${opts.cmd}: ${cause.message}`, cause);
		this.name = "DriverBootError";
		this.opts = opts;
	}
}

export class DriverPaneDeadError extends DriverError {
	public readonly target: Target;
	public readonly info: PaneInfo;

	constructor(target: Target, info: PaneInfo) {
		super(`pane ${info.paneId} is dead (last cmd: ${info.cmd}, pid: ${info.pid})`);
		this.name = "DriverPaneDeadError";
		this.target = target;
		this.info = info;
	}
}

export interface AssertionContext {
	target: Target;
	step: Step;
	expected: RegExp;
	actual: string; // last full capture before failure (visible region)
	scrollback: string; // wider window, includes history
	status: string; // bottom 3 lines (status / footer)
	priorSteps: Step[]; // breadcrumb of completed steps in this run
	durationMs: number;
}

export class DriverAssertionError extends DriverError {
	public readonly target: Target;
	public readonly step: Step;
	public readonly expected: RegExp;
	public readonly actual: string;
	public readonly scrollback: string;
	public readonly status: string;
	public readonly priorSteps: Step[];
	public readonly durationMs: number;

	constructor(ctx: AssertionContext) {
		const tail = ctx.actual.slice(-800);
		super(
			`assertion failed: expected /${ctx.expected.source}/ after ${ctx.durationMs}ms\n` +
				`step: ${JSON.stringify(ctx.step)}\n` +
				`--- last 800 bytes of pane ---\n${tail}`,
		);
		this.name = "DriverAssertionError";
		this.target = ctx.target;
		this.step = ctx.step;
		this.expected = ctx.expected;
		this.actual = ctx.actual;
		this.scrollback = ctx.scrollback;
		this.status = ctx.status;
		this.priorSteps = ctx.priorSteps;
		this.durationMs = ctx.durationMs;
	}

	/** JSON-serializable failure record — what the agent CLI returns. */
	toReport(): {
		kind: "assertion-failed";
		expected: string;
		actual: string;
		scrollback: string;
		status: string;
		priorSteps: Step[];
		durationMs: number;
	} {
		return {
			kind: "assertion-failed",
			expected: this.expected.source,
			actual: this.actual,
			scrollback: this.scrollback,
			status: this.status,
			priorSteps: this.priorSteps,
			durationMs: this.durationMs,
		};
	}
}

export class DriverIdleTimeoutError extends DriverError {
	public readonly target: Target;
	public readonly lastCapture: string;
	public readonly timeoutMs: number;

	constructor(target: Target, lastCapture: string, timeoutMs: number) {
		const tail = lastCapture.slice(-800);
		super(`waitIdle timed out after ${timeoutMs}ms\n--- last 800 bytes ---\n${tail}`);
		this.name = "DriverIdleTimeoutError";
		this.target = target;
		this.lastCapture = lastCapture;
		this.timeoutMs = timeoutMs;
	}
}
