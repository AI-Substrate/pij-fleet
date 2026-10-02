// Shared test helpers. Keep tiny — extract patterns only after they're
// duplicated across ≥2 test files.

import { createHash } from "node:crypto";
import { type RmOptions, rmSync } from "node:fs";

export interface AppendCall<T = unknown> {
	customType: string;
	data: T;
}

export function makeRecorder<T = unknown>() {
	const calls: AppendCall<T>[] = [];
	const append = (customType: string, data: unknown): void => {
		calls.push({ customType, data: data as T });
	};
	return { append, calls };
}

export function lastCustomType(calls: AppendCall[]): string | undefined {
	return calls.at(-1)?.customType;
}

export type RemoveTemporaryTree = (path: string, options: RmOptions) => void;

const TEMPORARY_TREE_REMOVE_OPTIONS = {
	recursive: true,
	force: true,
	maxRetries: 3,
	retryDelay: 100,
} satisfies RmOptions;

export function removeTemporaryTree(path: string, remove: RemoveTemporaryTree = rmSync): void {
	remove(path, TEMPORARY_TREE_REMOVE_OPTIONS);
}

// ─── FakeIterationRunner (Plan 008 T022) ─────────────────────────────
//
// Deterministic IterationRunner for `agentic-loops` extensions. Produces a
// fixed sequence of IterationResult values regardless of input. Used by:
//   - .pi/extensions/ralph-loop/smoke.ts under PIJ_RALPH_FAKE_RUNNER=1
//   - any future agentic-loops extension's store/runner tests
//
// Cross-domain helper (lives in `_platform`-tagged test infrastructure).
// Imports nothing from `.pi/extensions/*` to avoid circular deps; the
// IterationRunner contract is mirrored structurally below so extensions
// can adopt this without importing each other's types.

/**
 * Narrow shape of the agentic-loops `IterationResult` contract that this
 * helper produces. Mirrors `.pi/extensions/ralph-loop/store.ts` structurally
 * (P6) without importing it.
 */
export interface FakeIterationResult {
	readonly output: string;
	readonly taskTitle: string;
	readonly taskFingerprint: string;
	readonly costUsd: number | null;
	readonly durationMs: number;
	readonly verdict: "ok" | "agent_error" | "session_error";
	readonly errorDetail?: string;
}

export interface FakeIterationInputShape {
	readonly planSnapshot: string;
}

export interface FakeIterationRunnerOpts {
	/** Number of iterations before emitting the completion sigil. Default: 3. */
	readonly iterationsBeforeComplete?: number;
	/** Per-iteration cost in USD. Default: 0.0001 * iteration index. */
	readonly costPerIteration?: number;
	/** Completion sigil to emit on the final iteration. Default: workshop 001 sigil. */
	readonly completionSigil?: string;
}

const DEFAULT_SIGIL = "<promise>COMPLETE</promise>";

function fingerprint(title: string): string {
	return createHash("sha1").update(title.trim().toLowerCase(), "utf8").digest("hex").slice(0, 12);
}

/**
 * Deterministic 3-iteration (configurable) runner. Returns IterationResult
 * values shaped to satisfy the `IterationRunner` interface from
 * `agentic-loops` without importing it directly.
 */
export class FakeIterationRunner {
	private count = 0;
	private readonly cap: number;
	private readonly cost: number | undefined;
	private readonly sigil: string;

	constructor(opts: FakeIterationRunnerOpts = {}) {
		this.cap = opts.iterationsBeforeComplete ?? 3;
		this.cost = opts.costPerIteration;
		this.sigil = opts.completionSigil ?? DEFAULT_SIGIL;
	}

	async runIteration(input: FakeIterationInputShape): Promise<FakeIterationResult> {
		this.count++;
		// Cycle through the plan's undone tasks by iteration index so a smoke
		// driving 3 iterations sees Task one / Task two / Task three rather than
		// Task one repeated. The fake runner does NOT mutate the plan file; if
		// fewer undone tasks exist than iterations, repeat the last one.
		const allUndone = Array.from(input.planSnapshot.matchAll(/-\s+\[ \]\s+(.+?)\s*$/gm)).map(
			(m) => m[1]?.trim() ?? "",
		);
		const pick =
			allUndone.length > 0
				? (allUndone[Math.min(this.count - 1, allUndone.length - 1)] ?? `fake-task-${this.count}`)
				: `fake-task-${this.count}`;
		const output =
			this.count >= this.cap
				? `Marking complete now ${this.sigil}`
				: `Iteration ${this.count} did ${pick}`;
		return {
			output,
			taskTitle: pick,
			taskFingerprint: fingerprint(pick),
			costUsd: this.cost ?? 0.0001 * this.count,
			durationMs: 10,
			verdict: "ok",
		};
	}

	/** Inspect iteration counter — useful for test assertions. */
	get callCount(): number {
		return this.count;
	}

	reset(): void {
		this.count = 0;
	}
}
